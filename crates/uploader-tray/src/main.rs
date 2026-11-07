// トレイ常駐アプリ本体（トレイは後で作る）。
// いまはフォルダ監視の動作確認用: 引数で渡したフォルダを監視して、変更をコンソールに出す。
//
//   cargo run -p uploader-tray -- D:\share\

use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

use uploader_core::{plan, watcher};

fn main() {
    let root: PathBuf = match std::env::args().nth(1) {
        Some(arg) => arg.into(),
        None => {
            eprintln!("使い方: uploader-tray <監視フォルダ>");
            std::process::exit(2);
        }
    };

    // チャンネル: tx（送信側）は watcher に渡し、rx（受信側）はここで使う
    let (tx, rx) = mpsc::channel();

    // 戻り値を変数に持っておかないと、すぐ drop されて監視が止まる
    let _watcher = match watcher::watch(&root, Duration::from_millis(800), tx) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("監視を開始できませんでした: {e}");
            std::process::exit(1);
        }
    };
    println!("監視中: {} （Ctrl+C で終了）", root.display());

    // rx は、送信側がすべてなくなるまで、届いた変更を1つずつ返し続ける
    for change in rx {
        println!("{change:?}");

        // TODO(課題のあと): plan::plan(&root, change) を呼んで、Action を表示する
        for p in plan::plan(&root, change) {
            println!("{p:?}");
        }
    }
}
