//! ファイルの変更から「S3 に対して何をするか」を決める。
//! 要件メモの W-2（イベントと S3 操作の対応）と R-4（ルート直下は警告）に対応する。

use std::path::{Path, PathBuf};

use crate::change::FsChange;
use crate::mapping::{MapError, S3Target, map_path};

/// S3 に対して行う操作（または利用者への警告）。
#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    /// ローカルの `path` を読んで、`target` へアップロードする。
    Upload { path: PathBuf, target: S3Target },
    /// `target` を S3 から削除する。
    Delete { target: S3Target },
    /// アップロードできないことを利用者に知らせる。
    Warn { path: PathBuf, reason: MapError },
}

/// 1つの変更から、行う操作の一覧を作る。
///
/// ここを実装するのが課題。下の tests がすべて通れば完成。
///
/// ルール:
///   1. Created / Modified
///      - map_path が Ok なら Upload
///      - Err(NoBucketFolder) か Err(InvalidUnicode) なら Warn
///      - それ以外の Err（OutsideRoot, NoExtension）なら何もしない
///   2. Removed
///      - map_path が Ok なら Delete
///      - Err ならどれでも何もしない（消えたファイルについて警告しても意味がないので）
///   3. Renamed { from, to }
///      - from をルール2（削除）で、to をルール1（アップロード）で処理し、その順番で並べる
///
/// ヒント:
///   - enum の中身は `match` で取り出せる。`FsChange::Renamed { from, to } => ...` のように書く
///   - `Created` と `Modified` は同じ処理なので、`A(p) | B(p) => ...` と1つの腕にまとめられる
///   - Result も enum なので match できる。`Err(MapError::A | MapError::B) => ...` のように書ける
///   - ルール1・ルール2をそれぞれ小さな関数にしておくと、ルール3で使い回せる
///   - Vec に要素を足すのは `push`、別の Vec の中身をまとめて足すのは `extend`
///
/// 考えてみてほしいこと:
///   `change` を `&FsChange`（借用）ではなく `FsChange`（所有）で受け取っている。
///   こうすると、中の PathBuf を clone せずに Action へ「移動」できる。どこで効いてくるか探してみてください。
pub fn plan(root: &Path, change: FsChange) -> Vec<Action> {
    // todo!("ここを実装する")
    match change {
        FsChange::Created(path) | FsChange::Modified(path) => {
            let result = map_path(root, &path);
            upload(path, result)
        }
        FsChange::Removed(path) => {
            let result = map_path(root, &path);
            delete(result)
        }
        FsChange::Renamed { from, to } => {
            let from_result = map_path(root, &from);
            let to_result = map_path(root, &to);

            let mut del = delete(from_result);
            let up = upload(to, to_result);
            del.extend(up);
            del
        }
    }
}

fn upload(path: PathBuf, result: Result<S3Target, MapError>) -> Vec<Action> {
    match result {
        Ok(target) => {
            vec![Action::Upload { path, target }]
        }
        Err(reason @ (MapError::InvalidUnicode | MapError::NoBucketFolder)) => {
            vec![Action::Warn { path, reason }]
        }
        Err(_) => vec![],
    }
}

fn delete(result: Result<S3Target, MapError>) -> Vec<Action> {
    match result {
        Ok(target) => {
            vec![Action::Delete { target }]
        }
        Err(_) => vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROOT: &str = r"D:\share\";

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    fn t(bucket: &str, key: &str) -> S3Target {
        S3Target {
            bucket: bucket.to_string(),
            key: key.to_string(),
        }
    }

    fn run(change: FsChange) -> Vec<Action> {
        plan(Path::new(ROOT), change)
    }

    // --- ルール1: Created / Modified ---

    #[test]
    fn 作成はアップロード() {
        assert_eq!(
            run(FsChange::Created(p(r"D:\share\b\2026\a.png"))),
            vec![Action::Upload {
                path: p(r"D:\share\b\2026\a.png"),
                target: t("b", "2026/a.png"),
            }]
        );
    }

    #[test]
    fn 変更もアップロード() {
        assert_eq!(
            run(FsChange::Modified(p(r"D:\share\b\a.png"))),
            vec![Action::Upload {
                path: p(r"D:\share\b\a.png"),
                target: t("b", "a.png"),
            }]
        );
    }

    #[test]
    fn ルート直下に作成されたら警告() {
        assert_eq!(
            run(FsChange::Created(p(r"D:\share\x.png"))),
            vec![Action::Warn {
                path: p(r"D:\share\x.png"),
                reason: MapError::NoBucketFolder,
            }]
        );
    }

    #[test]
    fn 拡張子なしの作成は何もしない() {
        // フォルダを作ったときもこれになる
        assert_eq!(run(FsChange::Created(p(r"D:\share\b\newfolder"))), vec![]);
    }

    #[test]
    fn ルートの外は何もしない() {
        assert_eq!(run(FsChange::Modified(p(r"D:\other\b\a.png"))), vec![]);
    }

    // --- ルール2: Removed ---

    #[test]
    fn 削除は削除() {
        assert_eq!(
            run(FsChange::Removed(p(r"D:\share\b\a.png"))),
            vec![Action::Delete {
                target: t("b", "a.png")
            }]
        );
    }

    #[test]
    fn ルート直下の削除は何もしない() {
        assert_eq!(run(FsChange::Removed(p(r"D:\share\x.png"))), vec![]);
    }

    // --- ルール3: Renamed ---

    #[test]
    fn リネームは削除してからアップロード() {
        assert_eq!(
            run(FsChange::Renamed {
                from: p(r"D:\share\b\old.png"),
                to: p(r"D:\share\b\new.png"),
            }),
            vec![
                Action::Delete {
                    target: t("b", "old.png")
                },
                Action::Upload {
                    path: p(r"D:\share\b\new.png"),
                    target: t("b", "new.png"),
                },
            ]
        );
    }

    #[test]
    fn 別バケットへの移動() {
        assert_eq!(
            run(FsChange::Renamed {
                from: p(r"D:\share\b1\a.png"),
                to: p(r"D:\share\b2\sub\a.png"),
            }),
            vec![
                Action::Delete {
                    target: t("b1", "a.png")
                },
                Action::Upload {
                    path: p(r"D:\share\b2\sub\a.png"),
                    target: t("b2", "sub/a.png"),
                },
            ]
        );
    }

    #[test]
    fn 拡張子を消すリネームは削除だけ() {
        assert_eq!(
            run(FsChange::Renamed {
                from: p(r"D:\share\b\a.png"),
                to: p(r"D:\share\b\a"),
            }),
            vec![Action::Delete {
                target: t("b", "a.png")
            }]
        );
    }

    #[test]
    fn ルート直下からバケットへの移動はアップロードだけ() {
        // from 側は「削除」のルールなので警告は出ない
        assert_eq!(
            run(FsChange::Renamed {
                from: p(r"D:\share\x.png"),
                to: p(r"D:\share\b\x.png"),
            }),
            vec![Action::Upload {
                path: p(r"D:\share\b\x.png"),
                target: t("b", "x.png"),
            }]
        );
    }

    #[test]
    fn バケットからルート直下への移動は削除と警告() {
        assert_eq!(
            run(FsChange::Renamed {
                from: p(r"D:\share\b\x.png"),
                to: p(r"D:\share\x.png"),
            }),
            vec![
                Action::Delete {
                    target: t("b", "x.png")
                },
                Action::Warn {
                    path: p(r"D:\share\x.png"),
                    reason: MapError::NoBucketFolder,
                },
            ]
        );
    }
}
