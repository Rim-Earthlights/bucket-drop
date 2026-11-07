//! フォルダ監視。
//!
//! OS から届くファイルイベントを notify-debouncer-full でまとめ（デバウンス）、
//! このアプリ用の `FsChange` に変換してチャンネルへ送る。
//! 要件メモの W-1（サブフォルダも監視）と W-3（連続イベントをまとめる）に対応する。
//!
//! 構造:
//!
//! ```text
//!  OS ──イベント──▶ [notify のスレッド] ──▶ [debouncer のスレッド]
//!                                              │ to_changes() で変換
//!                                              ▼
//!                                       Sender<FsChange> ──▶ 受け取る側（main など）
//! ```
//!
//! イベントの処理は notify が用意した別スレッドで行われる。
//! そのため、受け取る側とはチャンネル（mpsc）でやりとりする。

use std::path::Path;
use std::sync::mpsc::Sender;
use std::time::Duration;

use notify_debouncer_full::notify::event::{ModifyKind, RenameMode};
use notify_debouncer_full::notify::{self, Event, EventKind, RecommendedWatcher, RecursiveMode};
use notify_debouncer_full::{DebounceEventResult, Debouncer, RecommendedCache, new_debouncer};

use crate::change::FsChange;

/// 監視中であることを表すハンドル。
///
/// **これを drop（変数のスコープを抜ける・破棄される）すると監視が止まる。**
/// 監視を続けたい間は、どこかで持ち続けておく必要がある。
/// これも所有権の応用で、「持ち主がいなくなったら後片付けする」という RAII の考え方。
pub struct FolderWatcher {
    // 外から触る必要はないが、持っていること自体に意味がある。
    // 先頭の `_` は「読まないけど意図して持っている」という印。
    _debouncer: Debouncer<RecommendedWatcher, RecommendedCache>,
}

/// `root` 以下の監視を始める。変更は `tx` に送られる。
///
/// `debounce` は、連続するイベントをまとめる待ち時間（要件では 500ms〜1s 程度）。
pub fn watch(root: &Path, debounce: Duration, tx: Sender<FsChange>) -> notify::Result<FolderWatcher> {
    // `move` クロージャ: tx の所有権をクロージャの中に移す。
    // クロージャは別スレッドで呼ばれるので、借用ではなく所有している必要がある。
    let handler = move |result: DebounceEventResult| match result {
        Ok(events) => {
            for event in events {
                // DebouncedEvent は Deref で Event として扱える（&*event）
                for change in to_changes(&event).into_iter().filter(is_not_directory) {
                    // 受け取る側が終了していたら送れないが、そのときは無視してよい
                    let _ = tx.send(change);
                }
            }
        }
        Err(errors) => {
            // TODO(X-5): バッファあふれなどで取りこぼしたときは再スキャンする
            for error in errors {
                eprintln!("[watcher] error: {error:?}");
            }
        }
    };

    let mut debouncer = new_debouncer(debounce, None, handler)?;
    debouncer.watch(root, RecursiveMode::Recursive)?;
    Ok(FolderWatcher { _debouncer: debouncer })
}

/// フォルダについての変更なら false を返す（filter で落とすため）。
///
/// フォルダ名にドットがあると（例: `my.bucket`, `2026.11`）拡張子付きのファイルと区別できないので、
/// 実際にディスクを見て判定する。消えたもの（Removed）は確認できないのでそのまま通す。
/// フォルダの削除・リネームを S3 にどう反映するかは要件メモの X-7（検討事項）。
fn is_not_directory(change: &FsChange) -> bool {
    match change {
        FsChange::Created(p) | FsChange::Modified(p) => !p.is_dir(),
        FsChange::Renamed { to, .. } => !to.is_dir(),
        FsChange::Removed(_) => true,
    }
}

/// notify のイベントを `FsChange` に変換する。
///
/// 1つのイベントに複数のパスが入っていることがあるので Vec で返す。
/// 関係ないイベント（属性の変更、アクセスなど）は空の Vec になる。
fn to_changes(event: &Event) -> Vec<FsChange> {
    let paths = &event.paths;

    match event.kind {
        EventKind::Create(_) => paths.iter().cloned().map(FsChange::Created).collect(),

        // リネーム（from と to の両方がわかっている）
        EventKind::Modify(ModifyKind::Name(RenameMode::Both)) if paths.len() == 2 => {
            vec![FsChange::Renamed {
                from: paths[0].clone(),
                to: paths[1].clone(),
            }]
        }
        // 片方しかわからないリネーム = 監視フォルダの外との移動
        EventKind::Modify(ModifyKind::Name(RenameMode::From)) => {
            paths.iter().cloned().map(FsChange::Removed).collect()
        }
        EventKind::Modify(ModifyKind::Name(RenameMode::To)) => {
            paths.iter().cloned().map(FsChange::Created).collect()
        }
        // どちらかわからないリネームは、今ファイルがあるかどうかで判断する
        EventKind::Modify(ModifyKind::Name(_)) => paths
            .iter()
            .cloned()
            .map(|p| if p.exists() { FsChange::Created(p) } else { FsChange::Removed(p) })
            .collect(),

        // 属性やタイムスタンプだけの変更はアップロード不要
        EventKind::Modify(ModifyKind::Metadata(_)) => Vec::new(),
        EventKind::Modify(_) => paths.iter().cloned().map(FsChange::Modified).collect(),

        EventKind::Remove(_) => paths.iter().cloned().map(FsChange::Removed).collect(),

        // Access（読み取り）など
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify_debouncer_full::notify::event::{CreateKind, DataChange, MetadataKind, RemoveKind};
    use std::path::PathBuf;

    fn ev(kind: EventKind, paths: &[&str]) -> Event {
        paths.iter().fold(Event::new(kind), |e, p| e.add_path(PathBuf::from(p)))
    }

    #[test]
    fn 作成() {
        let e = ev(EventKind::Create(CreateKind::File), &[r"D:\share\b\a.png"]);
        assert_eq!(to_changes(&e), vec![FsChange::Created(r"D:\share\b\a.png".into())]);
    }

    #[test]
    fn 変更() {
        let e = ev(EventKind::Modify(ModifyKind::Data(DataChange::Any)), &[r"D:\share\b\a.png"]);
        assert_eq!(to_changes(&e), vec![FsChange::Modified(r"D:\share\b\a.png".into())]);
    }

    #[test]
    fn 属性だけの変更は無視() {
        let e = ev(EventKind::Modify(ModifyKind::Metadata(MetadataKind::Any)), &[r"D:\share\b\a.png"]);
        assert_eq!(to_changes(&e), vec![]);
    }

    #[test]
    fn 削除() {
        let e = ev(EventKind::Remove(RemoveKind::File), &[r"D:\share\b\a.png"]);
        assert_eq!(to_changes(&e), vec![FsChange::Removed(r"D:\share\b\a.png".into())]);
    }

    #[test]
    fn リネーム() {
        let e = ev(
            EventKind::Modify(ModifyKind::Name(RenameMode::Both)),
            &[r"D:\share\b\old.png", r"D:\share\b\new.png"],
        );
        assert_eq!(
            to_changes(&e),
            vec![FsChange::Renamed {
                from: r"D:\share\b\old.png".into(),
                to: r"D:\share\b\new.png".into(),
            }]
        );
    }

    #[test]
    fn 外への移動は削除扱い() {
        let e = ev(EventKind::Modify(ModifyKind::Name(RenameMode::From)), &[r"D:\share\b\a.png"]);
        assert_eq!(to_changes(&e), vec![FsChange::Removed(r"D:\share\b\a.png".into())]);
    }

    #[test]
    fn 外からの移動は作成扱い() {
        let e = ev(EventKind::Modify(ModifyKind::Name(RenameMode::To)), &[r"D:\share\b\a.png"]);
        assert_eq!(to_changes(&e), vec![FsChange::Created(r"D:\share\b\a.png".into())]);
    }
}
