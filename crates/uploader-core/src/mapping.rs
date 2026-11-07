//! ローカルのファイルパスを S3 のバケット名とキーに変換する。
//! 要件メモ（docs/requirements.md）の R-1〜R-6 に対応する。
//!
//! ルール:
//!   監視ルート直下のフォルダ名がバケット名、その下の相対パスがキーになる。
//!   例: root = D:\share\、file = D:\share\my-bucket\2026\11\a.png
//!       → bucket = "my-bucket", key = "2026/11/a.png"

use std::path::Path;

/// 変換結果。アップロード先のバケット名とキー。
#[derive(Debug, PartialEq, Eq)]
pub struct S3Target {
    pub bucket: String,
    pub key: String,
}

/// 変換できなかった理由。
#[derive(Debug, PartialEq, Eq)]
pub enum MapError {
    /// ファイルが監視ルートの外にある。
    OutsideRoot,
    /// 監視ルート直下に置かれていて、バケットフォルダに入っていない（R-4）。
    NoBucketFolder,
    /// 拡張子が無い（R-6）。
    NoExtension,
    /// パスに UTF-8 として扱えない文字が含まれている。
    InvalidUnicode,
}

/// `file` を、監視ルート `root` を基準にバケット名とキーへ変換する。
///
/// ここを実装するのが課題。下の tests がすべて通れば完成。
///
/// エラーを判定する順番: ルートの外か（OutsideRoot）→ 拡張子があるか（NoExtension）
///                       → バケットフォルダに入っているか（NoBucketFolder）
///
/// 使えそうな道具（ヒント。全部使う必要はない）:
///   - `Path::strip_prefix`  … 先頭の root 部分を取り除く。末尾の `\` の有無は気にしなくてよい（なぜかも考えてみて）
///   - `Path::components`    … パスを部品ごとに分けたイテレータを返す
///   - `Path::extension`     … 拡張子を `Option<&OsStr>` で返す
///   - `OsStr::to_str`       … `&OsStr` を `Option<&str>` にする（UTF-8 でなければ None）
///   - `Vec<&str>` の `join("/")`
///   - `Option::ok_or(...)` と `?` 演算子 … None をエラーに変えて早期 return する
///
/// 引数が `&Path`（借用）で、戻り値が `String`（所有）になっている理由も考えてみてください。
pub fn map_path(root: &Path, file: &Path) -> Result<S3Target, MapError> {
    // メソッドチェーンで書いたほうがいいらしい
    // Err型を別の型にするのはmap_errらしい |_|はクロージャ
    // ?演算子はErr時に呼び出し元へErrを返す, OKのときは値を返す. つまり&Pathが取れる
    // let b_path = Path::strip_prefix(file, root);
    let b_path = file.strip_prefix(root).map_err(|_| MapError::OutsideRoot)?;
    file.extension().ok_or(MapError::NoExtension)?;

    let mut components = b_path.components();
    // 型を合わせる
    let bucket: String = components
        .next()
        .ok_or(MapError::NoBucketFolder)?
        .as_os_str()
        .to_str()
        .ok_or(MapError::InvalidUnicode)?
        .to_string();

    // 読むべきもの
    // https://doc.rust-lang.org/std/path/struct.Components.html
    // https://doc.rust-lang.org/std/ffi/struct.OsStr.html
    // https://doc.rust-lang.org/std/option/enum.Option.html
    // https://doc.rust-lang.org/std/result/enum.Result.html

    // https://doc.rust-lang.org/std/iter/struct.Map.html Mapだし参考になりそう

    // let mut parts: Vec<&str> = Vec::new();
    // for c in components {
    //     let value = c.as_os_str().to_str().ok_or(MapError::InvalidUnicode)?;
    //     parts.push(value);
    // }

    // componentsの各要素をstrにする (TypeScriptのmapと同じ)
    let parts: Result<Vec<&str>, MapError> = components
        .map(|component| {
            component
                .as_os_str()
                .to_str()
                .ok_or(MapError::InvalidUnicode)
        })
        .collect();

    // 型がResult<Vec<&str>, MapError>だから?演算子で取り出してからjoin
    let key = parts?.join("/");

    if key.is_empty() {
        return Err(MapError::NoBucketFolder);
    }

    // 型を合わせる Result<S3Target, MapError>
    Ok(S3Target { bucket, key })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// テストを短く書くための補助関数。
    fn map(root: &str, file: &str) -> Result<S3Target, MapError> {
        map_path(Path::new(root), Path::new(file))
    }

    fn target(bucket: &str, key: &str) -> Result<S3Target, MapError> {
        Ok(S3Target {
            bucket: bucket.to_string(),
            key: key.to_string(),
        })
    }

    // R-1: 直下フォルダがバケット、その下がキー
    #[test]
    fn バケット直下のファイル() {
        assert_eq!(
            map(r"D:\share\", r"D:\share\my-bucket\a.png"),
            target("my-bucket", "a.png")
        );
    }

    // R-1, R-2: 深い階層は `/` でつなぐ
    #[test]
    fn 深い階層のファイル() {
        assert_eq!(
            map(r"D:\share\", r"D:\share\my-bucket\2026\11\a.png"),
            target("my-bucket", "2026/11/a.png")
        );
    }

    // R-3: ルートの末尾に `\` が無くても同じ結果
    #[test]
    fn ルート末尾の区切りなし() {
        assert_eq!(
            map(r"D:\share", r"D:\share\my-bucket\2026\a.png"),
            target("my-bucket", "2026/a.png")
        );
    }

    // R-4: ルート直下のファイルは対象外
    #[test]
    fn ルート直下のファイルは対象外() {
        assert_eq!(
            map(r"D:\share\", r"D:\share\x.png"),
            Err(MapError::NoBucketFolder)
        );
    }

    // R-5: 日本語や空白はそのまま
    #[test]
    fn 日本語と空白を含む名前() {
        assert_eq!(
            map(
                r"D:\share\",
                r"D:\share\my-bucket\スクショ\日本語 ファイル.jpg"
            ),
            target("my-bucket", "スクショ/日本語 ファイル.jpg")
        );
    }

    // R-6: 拡張子なしは対象外
    #[test]
    fn 拡張子なしは対象外() {
        assert_eq!(
            map(r"D:\share\", r"D:\share\my-bucket\noext"),
            Err(MapError::NoExtension)
        );
    }

    // R-6: バケットフォルダそのもの（拡張子なし）も対象外
    #[test]
    fn バケットフォルダ自体は対象外() {
        assert_eq!(
            map(r"D:\share\", r"D:\share\my-bucket"),
            Err(MapError::NoExtension)
        );
    }

    // 監視ルートの外
    #[test]
    fn ルートの外は対象外() {
        assert_eq!(
            map(r"D:\share\", r"D:\other\my-bucket\a.png"),
            Err(MapError::OutsideRoot)
        );
    }

    // 名前が前方一致するだけの別フォルダはルートの外
    // （文字列の前方一致で判定するとここで引っかかる）
    #[test]
    fn 名前が似ている別フォルダ() {
        assert_eq!(
            map(r"D:\share", r"D:\share2\my-bucket\a.png"),
            Err(MapError::OutsideRoot)
        );
    }
}
