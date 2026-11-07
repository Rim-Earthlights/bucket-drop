//! ファイルシステムの変更を表す型。
//! watcher（OS のイベント）と plan（何をするかの判断）の間で受け渡す共通の型。

use std::path::PathBuf;

/// 監視フォルダで起きた変更。
///
/// OS や notify crate の細かいイベント種別を、このアプリで必要な4種類に整理したもの。
/// パスはすべて絶対パス。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FsChange {
    /// ファイル（またはフォルダ）が作られた。監視フォルダの外から移動してきた場合も含む。
    Created(PathBuf),
    /// ファイルの中身が変わった。
    Modified(PathBuf),
    /// ファイル（またはフォルダ）が消えた。監視フォルダの外へ移動した場合も含む。
    Removed(PathBuf),
    /// 監視フォルダの中でリネーム・移動された。
    Renamed { from: PathBuf, to: PathBuf },
}
