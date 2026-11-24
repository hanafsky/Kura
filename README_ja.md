# kura

Rust製の、ターミナル上で動作するシンプルな2ペインファイルマネージャーです。ratatuiとcrosstermを使用しています。

## 特徴

- 二画面のディレクトリ一覧表示
- Vimライクなキーバインド
- 反対側のペインでの画像プレビュー（png、jpeg、gif、bmp、tiff、webp。heic/avif は ffmpeg 経由）。Kitty / Sixel / iTerm2 の画像プロトコルに対応したターミナルでは高画質で表示
- 動画プレビュー：サムネイルと再生時間・コーデック・解像度などの情報（`ffmpeg` と `ffprobe` が必要）

## 対応環境

| OS | アーキテクチャ | リリースアセットの target |
| --- | --- | --- |
| macOS | Apple Silicon (arm64) | `aarch64-apple-darwin` |
| macOS | Intel (x86_64) | `x86_64-apple-darwin` |
| Linux / WSL2 | x86_64 | `x86_64-unknown-linux-musl` |
| Linux / WSL2 | aarch64 | `aarch64-unknown-linux-musl` |

Linux 向けバイナリは musl で静的リンクしているため、ディストリビューションを問わず、WSL2 上でもそのまま動作します。

Windows ネイティブは**対象外**です。Windows では WSL2 上で kura を使ってください。

## インストール

### ビルド済みバイナリ（GitHub Releases）

バイナリは [Releases ページ](https://github.com/hanafsky/Kura/releases) で公開されます。各リリースには、上表の target ごとに `kura-<target>.tar.gz`（アーカイブ）と `kura-<target>.tar.gz.sha256`（チェックサム）が添付されます。

`curl` でダウンロードし、チェックサムを検証してから展開します。

```bash
# 上表から自分の環境の target を選ぶ
TARGET=aarch64-apple-darwin

curl -LO "https://github.com/hanafsky/Kura/releases/latest/download/kura-${TARGET}.tar.gz"
curl -LO "https://github.com/hanafsky/Kura/releases/latest/download/kura-${TARGET}.tar.gz.sha256"

# チェックサムを検証（"kura-<target>.tar.gz: OK" と表示されれば成功）
shasum -a 256 -c "kura-${TARGET}.tar.gz.sha256"    # macOS
# sha256sum -c "kura-${TARGET}.tar.gz.sha256"      # Linux / WSL2

# 展開（アーカイブには kura-<target>/ ディレクトリが入っています）
tar -xzf "kura-${TARGET}.tar.gz"

# PATH の通ったディレクトリにバイナリを置く
mkdir -p ~/.local/bin
mv "kura-${TARGET}/kura" ~/.local/bin/
```

`~/.local/bin`（または配置先に選んだディレクトリ）が `PATH` に含まれていることを確認してください。

#### macOS：ブラウザでダウンロードした場合

`curl` でダウンロードしたファイルには quarantine 属性が付かないため、上記の手順では追加の操作は不要です。ブラウザでアーカイブをダウンロードした場合は、kura が署名されていないため macOS の Gatekeeper により実行がブロックされることがあります。展開したバイナリから quarantine 属性を外してください。

```bash
xattr -d com.apple.quarantine ~/.local/bin/kura
```

（`No such xattr` と表示された場合は属性が付いていないので、そのままで問題ありません。）

### Cargo でソースからビルド

Rust ツールチェーン（[rustup](https://rustup.rs/) など）が入っていれば、次のコマンドで導入できます。

```bash
cargo install --git https://github.com/hanafsky/Kura
```

`kura` は `~/.cargo/bin` にインストールされます。

### ffmpeg（任意）

動画プレビューと HEIC/AVIF 画像の表示には `ffmpeg` と `ffprobe` が必要です（どちらも `ffmpeg` パッケージに含まれます）。

```bash
# macOS（Homebrew）
brew install ffmpeg

# Debian / Ubuntu / WSL2
sudo apt install ffmpeg
```

## キー操作

- `j` / `k`: 現在のペイン内で下／上に移動（テキストビューアモード時はスクロール）。数値プレフィックス対応（例: `4j` で4行下に、`3k` で3行上へ移動）。テキストビューアモードでは、相対行番号を表示し、プレフィックス数の目安とします。
- `gg`: ファイル一覧またはテキストビューアの先頭へ移動（`0` プレフィックス + `j` と同等）。
- `G`: ファイル一覧またはテキストビューアの末尾へ移動（大きなプレフィックス + `j` と同等）。
- `V`: Visual Lineモードのようなマルチ選択モードに入ります。`j`/`k`で範囲を選択し、`Esc`または`V`で終了（選択状態は保持）。

- **左ペイン:**
  - `h`: 親ディレクトリへ移動
  - `l`: 右ペインへ移動

- **右ペイン:**
  - `h`: 左ペインへ移動
  - `l`: 親ディレクトリへ移動

- `Enter`: 選択したディレクトリに入る。テキストファイル選択時はテキストビューアを、画像・動画ファイル選択時は反対側のペインにプレビューを表示。再度`Enter`（プレビュー中は`Esc`も可）でファイルマネージャに戻ります。
- `o`: 選択中のエントリを既定のアプリで開きます（macOS では `open`）。動画を QuickTime で再生する場合などに使います。ファイル一覧・プレビュー中のどちらでも使えます。
  WSL では `wslpath -w` で変換したパスを `explorer.exe` で開きます。環境変数 `KURA_OPENER` を設定すると任意のプログラムで開けます。値は空白で区切られ、`<program> <args...> <path>` の形で実行されます（例: `KURA_OPENER="code -r"` → `code -r <path>`）。
- `e`: 選択中のファイルまたはディレクトリをエディタで開きます（`$VISUAL`、未設定なら `$EDITOR`、どちらもなければ `vi`）。値は `KURA_OPENER` と同様に空白で区切られます（例: `EDITOR="code -w"`）。エディタの実行中は kura の画面を抜け、終了すると画面を戻して一覧を再読み込みします。
- `v`: ファイルマネージャモードで現在のエントリを選択／解除します。選択されたエントリは `*` でマークされます。
- `y`: カーソル行またはマークされたすべてのエントリをクリップボードにコピーします。
- `p`: コピーしたエントリを現在のディレクトリに貼り付けます。
- `Y`: カーソル行またはマークされたすべてのエントリを、Claude Code などのコーディングエージェント向けの `@path` 形式でシステムのクリップボードにコピーします（例: `@src/main.rs @"docs/my notes.md"`）。パスは kura を起動したディレクトリからの相対パスで（その外にあるパスは絶対パス）、空白で区切られます。スペースを含むパスは `@"..."` のように引用符で囲みます。
  書き込みは OSC 52 エスケープシーケンスで行います（tmux・herdr・ssh 越しでも届きます）。kura からは端末が OSC 52 を受け付けたか判別できないため、`pbcopy`（macOS）、`wl-copy` / `xclip`（Linux）、`clip.exe`（WSL）にも書き込みます。ただし ssh 越し（`SSH_CONNECTION` / `SSH_TTY` / `SSH_CLIENT` のいずれかが設定されている）では OSC 52 だけを使います。tmux 内では `~/.tmux.conf` に `set -g set-clipboard on` を設定してください。`KURA_CLIPBOARD` には `auto`（既定。上記の動作）・`osc52`（OSC 52 のみ）・`native`（ネイティブのツールのみ）を指定できます。`KURA_AT_PATH=absolute` で常に絶対パスにします。
- `M`: 現在のペインで、git の変更があるエントリ（とそれを含むディレクトリ）だけを表示するかを切り替えます。
- `.`: 現在のペインで隠しファイル（`.` で始まる名前）を表示するかを切り替えます（既定は表示）。隠している間はペインのタイトルに `[no hidden]` と表示します。
- `i`: 現在のペインで、`.gitignore` などで git に無視されるエントリを隠すかを切り替えます（git リポジトリ内でのみ有効）。有効な間はペインのタイトルに `[no ignored]` と表示します。判定はディレクトリの読み込み時に取得した git status（`--ignored`）を使い、切り替えのたびに git を起動しません。無視されたディレクトリ（`target/` など）の中に入ったときは、中身がすべて無視対象なので何も隠しません。`.git` は無視対象ではないため `i` では隠れません（`.` で隠せます）。
  `.`・`i`・`M` は組み合わせて使えます。フィルタで隠れたエントリのマークは外れ、操作の対象になりません。
- `D`: 左右のペインのカーソル下のファイル同士の差分（`git diff --no-index`）を表示します。main のチェックアウトとエージェントの worktree にある同じファイルを比べる、といった使い方を想定しています。削除行は赤、追加行は緑、ハンクの見出し（`@@`）はシアンで表示します。テキストビューアと同じく `j` / `k`、数値プレフィックス、`gg` / `G` でスクロールでき、`Enter`・`q`・`Esc` でファイル一覧に戻ります。アクティブなペインのカーソルだけが git リポジトリ内のファイルを指している場合（反対側のカーソルがディレクトリの場合など）や、左右がリポジトリ内の同じファイルを指している場合（起動直後など）は、そのファイルの `HEAD` との差分（`git diff HEAD`）を表示します。アクティブ側のカーソルがディレクトリを指している、左右がリポジトリ外の同じファイルを指している、差分がない、といった理由で表示できない場合は、ステータスラインに理由を表示します。
- `/`: 検索モードに入ります。クエリを入力すると、ファイルリスト内の一致するエントリ（大文字小文字を区別しない部分一致）にジャンプします。カーソル位置から一覧全体を探し、末尾まで行くと先頭に戻ります。EnterまたはEscで検索を終了します。
- `n` / `N`: 最後に検索した語の次／前の一致へ移動します（末尾・先頭で折り返します）。
- `r`: 選択中のファイル名を変更します。下部にリネーム用プロンプトが表示されます。
- `s`: ソートモードを開き、ポップアップで最終更新日、作成日、ファイル容量、名前順を選択できます。
- `x`: カーソル行またはマークされたすべてのエントリを OS のゴミ箱へ移します。確認ダイアログが表示されます（`y` で実行、`n` / `Esc` / `Enter` / `q` で取り消し）。ゴミ箱が使えない環境（WSL や一部の Linux など）ではステータスラインにエラーを表示し、削除は行いません。
  kura からゴミ箱の一覧表示や復元はできません。macOS では Finder でゴミ箱を開き、項目をドラッグして戻してください（「戻す」が表示される場合はそれも使えます）。
- `X`: カーソル行またはマークされたすべてのエントリをディスクから完全に削除します。「Confirm Permanent Deletion」ダイアログで確認します。元に戻せません。
- `q`: アプリケーションを終了します。

## カラーマップ

- **青**: ディレクトリ
- **赤**: 隠しアイテム（`.`で始まるファイル・ディレクトリ）
- **緑**: 実行可能ファイル
- その他: 標準色

git リポジトリ内では、各名前の前にステータス記号を表示します：`M` 変更、`A` 追加、`D` 削除（削除済みファイルは行がないため、親ディレクトリに `+` が付きます）、`R` 名前変更、`U` コンフリクト、`?` 未追跡、`!` 無視、`+` 配下に変更があるディレクトリ。現在のブランチ名はステータスラインに表示します。状態はディレクトリの読み込み時（移動・貼り付け・削除・リネーム）にだけ取得し、常時は更新しません。

## 使用方法

```bash
cargo run
```

動画プレビューと HEIC/AVIF 画像の表示には `ffmpeg` が必要です（[ffmpeg（任意）](#ffmpeg任意) を参照）。

画像の表示方式は自動判定され、プレビュー下部に表示されます。強制したい場合は環境変数 `KURA_IMAGE_PROTOCOL` に `kitty` / `sixel` / `iterm2` / `halfblocks` を指定してください。

### コマンドライン引数

```
kura [OPTIONS] [LEFT] [RIGHT]
```

- `LEFT` / `RIGHT`: 左右のペインで開くディレクトリ。1つだけ指定すると左ペインに使われます。省略したペインはカレントディレクトリで開きます。
- `--last-dir-path <FILE>`: 終了時に、アクティブなペインのディレクトリを `FILE` に書き出します（下記参照）。
- `-h`, `--help` / `-V`, `--version`

### 終了時にディレクトリを移動する（cd-on-exit）

プログラムからは親シェルのカレントディレクトリを変えられないため、ラッパー関数を使います。`--last-dir-path` を付けて kura を起動し、終了時にアクティブだったディレクトリへ `cd` します。

```bash
# ~/.bashrc または ~/.zshrc
k() {
    local tmp dir
    tmp="$(mktemp)" || return
    command kura --last-dir-path "$tmp" "$@"
    dir="$(cat -- "$tmp")"
    rm -f -- "$tmp"
    if [ -n "$dir" ] && [ -d "$dir" ] && [ "$dir" != "$PWD" ]; then
        cd -- "$dir" || return
    fi
}
```

```fish
# ~/.config/fish/functions/k.fish
function k
    set -l tmp (mktemp); or return
    command kura --last-dir-path $tmp $argv
    set -l dir (cat -- $tmp)
    rm -f -- $tmp
    if test -n "$dir"; and test -d "$dir"; and test "$dir" != "$PWD"
        cd $dir
    end
end
```
