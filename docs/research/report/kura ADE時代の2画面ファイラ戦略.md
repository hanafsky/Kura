# kura を「エージェントの隣の2画面」に育てる

**結論から言うと、差別化の方向性は妥当です。ただし「Rust製・Vimキー・2画面TUI」だけでは差別化になりません。** 2026年には norte、che、rcmd、Directory Navigator、pelorus など小規模な2画面TUIが次々に登場しました。なかでも **norte はすでに「エージェント時代のファイルコマンダー」を名乗り、`norte mcp serve` で MCP サーバーを提供しています**（[norte README](https://github.com/compilando/norte)）。一方 Herdr では、メンテナがファイルパネルの内蔵を明確に断り、yazi や lf などの外部TUIを使うよう案内しています（[herdr#131](https://github.com/herdrdev/herdr/issues/131)）。Herdr 向けのファイラ連携は yazi と nnn 向けしかなく、2画面コマンダーは一つもありません（[awesome-herdr](https://github.com/yigitkonur/awesome-herdr/blob/main/catalog/06-worktrees-and-terminal.md)）。そのため狙うべき位置は「MCP経由でエージェントにファイルを触らせる道具」（norte の領域）ではなく、**「人間がエージェントの横で使う2画面の操作盤」** です。具体的には、エージェントが変更したファイルを片側に、worktree をもう片側に並べ、`@path` をワンキーでエージェントに送れることです。yazi についてのユーザーの認識はおおむね正しいものの、「ツリー型」ではなく **Miller カラム型** です。そして yazi の本当の脅威はレイアウトではなく、エージェント連携プラグインの層の厚さにあります。今の kura に最も足りないのは機能より **安全性** です。**同じディレクトリに `y`→`p` するとファイルが0バイトに切り詰められる** データ消失バグがあり、何よりも先に直す必要があります。この報告の後半では、v0.2.x（安全性とCI）、v0.3（ADE対応の中核）、v0.4（ADE連携）、それ以降、という順のロードマップを Issue 単位で示します。なお、git log では最初のコミットは「約2年前」ではなく **2025-05-31**（約1年半前）です。

## yazi は「ツリー」ではなく Miller カラムで、2画面の空席は本物

前提を一つ補足します。kura の最初のコミットは 2025-05-31 です。v0.1.9（2025-06-02）の後は約17か月の空白があり、2026-11-23 に v0.2.0 まで一気に進みました。「知らない言語でAIバイブコーディングによりTUIを作る」という当初の実証は、この短い2回の集中期間で成立したと言えます。

ユーザーの見立てを検証すると、**yazi は ranger 系の Miller カラム（親・現在・プレビューの3列）+ タブ** のマネージャーです（[tecmint](https://www.tecmint.com/yazi-terminal-file-manager-linux/)）。常時表示される独立した2つのディレクトリ一覧の間で F5/F6 的にコピーや移動をする、正統派の2画面モードは持っていません。vifm から移ってきたユーザーが2画面ワークフローを求めた議論でも、回答は「タブ間のコピー&ペーストで代用できる」でした（[yazi discussion #429](https://www.github.com/sxyazi/yazi/discussions/429)）。`toggle-pane.yazi` やレイアウト比率の変更で近い見た目は作れますが、2画面化プラグインとして知られたものは見つかりませんでした（[yazi-rs/plugins](https://github.com/yazi-rs/plugins)）。つまり **「yazi は Vim 親和性は似ているが別物」という認識は正しく、表現だけ「ツリー型」から「Miller カラム型」に直すのが正確** です（tree 型なのは broot です）。

人気の差は大きく開いています。2026-11-23 時点の GitHub では yazi が約4.36万スター、superfile が約2.41万、nnn が約2.21万、ranger が約1.75万、broot が約1.32万、lf が約9.6千です。これに対し正統派2画面の mc は1,057（GitHub リポジトリは2025年作成）、far2l は2,244 にとどまります（[yazi](https://github.com/sxyazi/yazi)、[mc](https://github.com/MidnightCommander/mc)、[far2l](https://github.com/elfmz/far2l)）。2画面というパラダイム自体が端末ユーザーの主流ではない、という事実は直視すべきです。そのうえで「主流が取り残した形式を ADE という新しい文脈で復活させる」と考えるなら、kura の戦略は筋が通ります。

## 既存ファイラは「非同期・VFS・外部制御」へ、新興2画面は「エージェント」へ動いている

**yazi** はリリースの頻度が高く、2026年の v26.x 系でゴミ箱、ドラッグ&ドロップ、コマンドパレットを兼ねるヘルプ、ダーク/ライトの自動切替、SFTP VFS、Zellij 内での Kitty 画像、「アクションを実行して結果を待つ」`ya exec` サブコマンドなどを追加しました（[yazi CHANGELOG](https://github.com/sxyazi/yazi/blob/main/CHANGELOG.md)）。DDS という pub/sub の IPC（`ya emit`、`ya pub-to`、イベントの JSON 出力）を土台に（[yazi DDS docs](https://yazi-rs.github.io/docs/dds)）、コミュニティがエージェント連携を次々に作っています。herdr のエージェントペインにパスを入れる `herdr-yazi-agent`（[GitHub](https://github.com/timofey-TK/herdr-yazi-agent)）、選択ファイルを渡して Claude Code や Codex を起動する `agent-tui.yazi`（[GitHub](https://github.com/tr1v3r/agent-tui.yazi)）、Claude Code の `/ide` プロトコルでホバー中のファイルを伝える `yazi-claude-ide`（[GitHub](https://github.com/FunnyQ/yazi-claude-ide)）などです。**「send-to-agent」はすでに yazi のプラグインで実現されている** ので、kura はこれを同等以上の手軽さで出さないと比較の土俵に乗れません。

**superfile**（Go）は v1.6.1（2026-11-20）で全OS対応のネイティブゴミ箱、複数選択の `--chooser-file`、cd-on-quit 関数を整えました（[release v1.6.1](https://github.com/yorukot/superfile/releases/tag/v1.6.1)）。**nnn** は v5.3（2026-08）でネイティブのプレビューペインと、tmux 上の Neovim ペインでファイルを開く `tnp` プラグインを追加しました（[release v5.3](https://github.com/jarun/nnn/releases/tag/v5.3)）。**lf** は r42 でクライアント/サーバーのソケットを強化し（[release r42](https://github.com/gokcehan/lf/releases/tag/r42)）、**broot** は v1.62.0 で `--listen`/`--send` を Windows でも動くようにしました（[broot CHANGELOG](https://github.com/Canop/broot/blob/main/CHANGELOG.md)）。主要ファイラが共通して強化しているのは「外部から操作できるソケットやCLI」であり、これは ADE 時代の必須部品と言えます。

正統派コマンダーの動きは遅めです。**mc** の確認できる最新安定版は 4.8.33（2025-01-23）で、4.8.34 は NEWS ページがあるもののリリースは未確認です（[NEWS-4.8.34](https://midnight-commander.org/wiki/NEWS-4.8.34?version=18)）。**far2l** は v_2.9.1（2026-10-07）が出ていますが、今回の調査では2025〜2026年の機能一覧までは抽出できていません（[far2l releases](https://github.com/elfmz/far2l/releases)）。mc 系は SSH 越しやコンソールで動く強さがある一方、ターミナルエミュレータに F キーを横取りされる問題が昔から指摘されています（[TechRadar](https://techradar.com/news/software/applications/6-best-orthodox-file-managers-for-linux-605506/3)）。Herdr のようにプレフィックスキーや修飾キーを奪い合うマルチプレクサの中では、**F キー依存ではない Vim キーの2画面であること自体が小さな利点** になります。

ユーザーが「martha」「folklyft」と呼んだものは、それぞれ **Marta**（marta.sh、Yan Zhulanow による macOS ネイティブの「two-panel file commander」）（[Introducing Marta](https://marta.sh/blog/introducing-marta)）と **ForkLift**（BinaryNights のプロプライエタリな macOS 2画面ファイラ）（[Wikipedia](https://en.wikipedia.org/wiki/ForkLift_(file_manager))）と特定できます。「folklyft」という綴りに完全一致する製品は見つかりませんでした。カタカナの「フォークリフト」をローマ字にした綴りと考えるのが自然です。Marta の changelog の最新は 0.8.2（2024-07）で動きは鈍く、（[changelog](https://marta.sh/changelog/)）、ベータ後は有料化すると予告しています（[back on track](https://marta.sh/blog/back-on-track/)）。ForkLift はタブ、フォルダ同期、SFTP/S3 などを持つ成熟製品ですが、最新版の番号は情報源によって 4.4.5 と 4.7.7 で食い違っています。どちらも **macOS 専用の GUI アプリなので、kura の直接の競合ではなく UX の参照先** です。キーボードだけでの2画面操作、プレビュー、同期といった体験の水準を測る物差しとして使えます。

最も注目すべきは2026年に現れた小さな2画面TUI群です。**norte**（Rust、2026-07作成、7スター）は「人間とファイルを触る AI エージェントのための2画面・正統派ファイルマネージャ」を掲げています。ヘッドレスコア + TUI/GUI/CLI、`norte mcp serve`（付与したスコープ内で Claude Code や Codex に作業させる）、ハッシュチェーンの操作ジャーナル、`vim` を含む7種のキーマッププリセット、WASM プラグインを備えます。対応プラットフォームは Linux と Windows プレビューで、macOS は挙げられていません（[norte README](https://github.com/compilando/norte)）。**termide**（189スター）はエディタ、2画面ファイラ、ターミナル、内蔵エージェントと MCP を1つの TUI にまとめています（[GitHub](https://github.com/termide/termide)）。pelorus はジョブキューや git グリフ、`$EDITOR` での一括リネームを持ちます（[GitHub](https://github.com/mogglemoss/pelorus)）。調査メモの一部には「MCP を公開するファイラは見つからない」という記述もありましたが、**norte という具体的な反例があるので、kura が MCP で一番乗りという想定は成り立ちません**。

## Herdr の空席と、kura が取るべきポジション

ADE 側の状況は kura に有利です。Herdr は「コーディングエージェントのためのランタイム」で、Rust の単一バイナリとして動きます。ペインごとにエージェントの状態（working/blocked/idle）を表示し、ソケット API（Unix ソケット上の NDJSON）で `pane.send_text`、`agent.prompt`、`events.subscribe`、`worktree.list` などを公開しています（[Herdr socket API](https://herdr.dev/docs/socket-api/)）。plugin v1 は「任意の argv コマンド + `herdr-plugin.toml`」という軽い仕組みで、`[[panes]]` に `placement = "split"` や `"popup"` を書くだけで kura をペインとして起動できます。`[[events]] on = "worktree.created"` のようなフックも書けます。`herdr-plugin` トピックを付けた公開リポジトリは30分ごとにマーケットプレイスへ索引されます（[Herdr plugins docs](https://herdr.dev/docs/plugins/)）。メンテナがファイルパネルの内蔵を断り外部TUIに委ねていること（[herdr#131](https://github.com/herdrdev/herdr/issues/131)）と、既存のファイラ系 Herdr プラグインに2画面がないこと（[awesome-herdr](https://github.com/yigitkonur/awesome-herdr/blob/main/catalog/06-worktrees-and-terminal.md)）を合わせると、**「Herdr 上の2画面ファイラ」という枠は実在し、まだ空いています**。ただし「最初のファイラ系プラグイン」ではなく「最初の2画面ファイラ」と言うのが正確です。

**Orca**（stablyai/orca、MIT）は事情が違います。git status で色分けされ、リアルタイムに更新される GUI のファイルエクスプローラーを内蔵しており、ファイルをエージェントの端末にドロップするとプロンプトにパスが貼られます（[Orca file explorer docs](https://orca.dev/docs/editing/file-explorer)）。441個あるプラグインの中にファイルマネージャはありません（[orca.dev/plugins](https://orca.dev/plugins)）。しかし Orca 自身のエクスプローラーと機能が重なるため、Orca での kura の役割は「Orca の端末ペインで動くキーボード専用の2画面TUI」に限られます。そこで出せる付加価値は、worktree 同士のコピーと比較、キーボードだけの操作です。Orca の端末エンジンが Kitty graphics に対応しているかは未確認です。

もう一つの経路が **Claude Code** です。`@` によるファイル参照は相対パスでも絶対パスでも複数でも受け付け、Claude は「ツール呼び出しのたびにファイルを読み直す」ので、外部で行った編集も拾われます（[Claude Code common workflows](https://code.claude.com/docs/en/common-workflows)）。フックには `PostToolUse`（`tool_input.file_path` を含む）、`FileChanged`、`WorktreeCreate` などがあり（[Claude Code hooks](https://code.claude.com/docs/en/hooks)）、`Edit|Write` 後にファイルパスをログに追記させれば、kura がそれを読んで「エージェントが変更したファイル」のビューを作れます。IDE 連携プロトコル（`~/.claude/ide/<port>.lock` + WebSocket MCP + `at_mentioned` 通知）を実装すれば、どの ADE の中にいても選択ファイルを Claude Code に `@mention` できます。ただしこれは **Anthropic が公式に文書化していない、claudecode.nvim から読み解いたプロトコル** なので（[claudecode.nvim README](https://raw.githubusercontent.com/coder/claudecode.nvim/main/README.md)）、壊れる前提のオプション機能として扱うべきです。

これらの材料から、kura のポジションは次のようにまとめられます。**norte が「エージェントにファイル操作を委ねる統制の層（MCP + ジャーナル）」なら、kura は「人間がエージェントの仕事を見て、渡して、取り込む2画面の操作盤」です。** 2画面であることが最も効くのは worktree の比較で、左に main、右にエージェントの worktree を置き、カーソル下の2ファイルの diff をワンキーで見られます。調査した範囲では、これを単一のアクションとして提供するツールはありませんでした。加えて kura にはすでに具体的な強みがあります。macOS ファーストであること（norte の README は macOS を挙げていません）、Herdr 内で Kitty 画像と動画サムネイルのプレビューが動くこと、macOS の入力ソースを ASCII に切り替える処理です。これらは日本語話者で Herdr を使う macOS ユーザーという、作者自身の利用条件にぴったり合っています。

Herdr の画像表示についても補足します。調査メモでは、ユーザーの Herdr が 0.7.4 で upstream（v0.9.5、2026-10-14）より遅れていること、v0.9.2 で独自の `pane.graphics.*` API を廃止して標準の Kitty graphics に一本化したこと、v0.9.4 で画像対応の広告の挙動を変えたことを確認しました（[herdr releases](https://github.com/herdrdev/herdr/releases)）。Kitty の仕様は、端末はクエリに「他の入力を処理せず即座に応答しなければならない」と定めているので（[Kitty graphics protocol](https://sw.kovidgoyal.net/kitty/graphics-protocol/)）、DSR の後に Kitty の応答が返る順序は Herdr 側の仕様違反の可能性があります。**kura 側の回避策はコミット 1798f55 で実装済み** です（Kitty の再プローブ、残った応答の読み捨て、`KURA_IMAGE_PROTOCOL` による強制指定。`src/graphics.rs`）。残る作業は、Herdr 0.9.x で再テストし、まだ再現すれば Herdr や ratatui-image に upstream 報告することだけです。

## 今の kura に足りないもの：機能より先に安全性

kura v0.2.0 は約1,580行・10ファイルの同期・シングルスレッドの2画面ブラウザで、メディアプレビュー層（Kitty/Sixel/iTerm2/halfblocks の判定、HEIC/AVIF の ffmpeg フォールバック、動画サムネイル）は完成度が高い一方、ファイル操作層が危険です（以下のコードの行番号はコミット d0cd342 時点のものです。未コミットの fmt 修正により、`input.rs` と `ui.rs` では数行ずれている可能性があります）。**`paste()` は貼り付け先がコピー元と同じかどうかを確かめずに `fs::copy(src, &dst)` を呼ぶため（`src/actions.rs:36-53`）、同じディレクトリで `y`→`p` するとファイルが0バイトになります。** 調査担当者が macOS 上で `std::fs::copy("a.txt","a.txt")` が `Ok(0)` を返しファイルを空にすることを実験で確認しています。両ペインは起動時に同じ cwd を開くので（`src/app.rs:52-55`）、初めて触った人が最初に試す操作で起きます。ディレクトリでは `copy_dir_recursively(src, src)` がツリー全体を切り詰め、自分の子ディレクトリへのコピーは無限再帰になります（`src/fs_utils.rs:73-85`）。ほかにも危ない点が重なっています。確認なしの上書き、確認なしで `remove_dir_all` を実行する `X`、「(y/N)」と表示しながら Enter で削除を実行する確認ダイアログ、`eprintln!` によるエラー出力（代替画面の裏に消える）、panic フックがないこと、権限エラーでもディレクトリ移動を確定してしまう `on_enter` です。また、マウスキャプチャを有効にしているのにマウスイベントを処理しないため（`src/main.rs:26`）、端末本来のテキスト選択が使えなくなっています。

ADE 用途の観点で欠けているものは、構造的な前提とその上に乗る機能の2層に分かれます。前提の層では、**マークがインデックス（`HashSet<usize>`）で保持され、refresh のたびに消える**（`src/app.rs:12,32`）ことが致命的です。このままファイル監視を入れると、エージェントがファイルを書くたびに人間の選択が別のファイルにずれます。キー処理、モード遷移、ファイル操作が `input.rs` の一つの関数に混在していて、キーマップ設定、IPC、テストのどれにも同じ「コマンド」を再利用できません。機能の層では、ステータスライン、ファイル監視、git status/diff、worktree の切替、`@path` のコピーと送信、`$EDITOR` で開く、cd-on-exit、ゴミ箱、隠しファイルと .gitignore の切替、ファジージャンプが欠けています。比較対象の水準は、yazi のゴミ箱・タスク進捗・プラグイン（[yazi GitHub](https://github.com/sxyazi/yazi)）、broot の組み込み git status（[broot](https://dystroy.org/broot/)）、lf の `watch` と `-remote`（[lf doc.md](https://github.com/gokcehan/lf/blob/master/doc.md)）です。実装ではそれぞれ、`notify` crate（FSEvents/inotify。ただし WSL2 の `/mnt/c` ではイベントが来ないことがある）（[docs.rs notify](https://docs.rs/notify/latest/notify/)）、`trash` crate（macOS では一覧と復元ができない）（[docs.rs trash](https://docs.rs/trash/latest/trash/)）、fuzzy マッチャーの nucleo（[GitHub](https://github.com/helix-editor/nucleo)）が候補になります。

クロスプラットフォームでは、WSL でファイルを開く処理が抜けています。`open_external` は macOS 以外では `xdg-open` を呼びますが（`src/media.rs`）、WSL では Windows アプリに届きません。`wslview`（wslu）は upstream が開発を終了しているので（[Ubuntu archive bug](https://lists.snapcraft.io/archives/ubuntu-archive/2025-December/113453.html)）、WSL を検出して `explorer.exe "$(wslpath -w …)"` を使うのが確実です（[MS Learn: WSL interop](https://learn.microsoft.com/en-us/windows/dev-environment/wsl-interop)）。画像は Windows Terminal 1.22 以降なら Sixel で表示できるはずです（[WT 1.22 blog](https://devblogs.microsoft.com/commandline/windows-terminal-preview-1-22-release)）。

CI とリリースは **進行中** です。調査の時点では release.yml しかなく、fmt は失敗し、clippy の警告が1件あり、テストは2件だけでした。その後、`cargo fmt` の適用と clippy 警告（`ui.rs` の不要なキャスト）の修正が済んでいます。新しい `.github/workflows/ci.yml`（ubuntu-24.04 と macos-latest で fmt/clippy/test）と、書き直した `release.yml` も用意されています。release.yml は aarch64/x86_64-apple-darwin を macos-latest で、x86_64/aarch64-unknown-linux-musl を ubuntu-24.04 と ubuntu-24.04-arm でビルドし、tar.gz と sha256 を出力し、タグとバージョンの一致を検査し、`softprops/action-gh-release@v3` で公開します。構成は yazi や ripgrep の慣行に沿っています（[yazi draft.yml](https://raw.githubusercontent.com/sxyazi/yazi/main/.github/workflows/draft.yml)、[ripgrep release.yml](https://raw.githubusercontent.com/BurntSushi/ripgrep/master/.github/workflows/release.yml)）。ただし **まだ push しておらず、GitHub 上での動作も確認していません**。また新しい release.yml には Windows ターゲットがないので、README で対応プラットフォーム（macOS + Linux/WSL）を明記する必要があります。ブラウザでダウンロードした未署名バイナリは Gatekeeper に止められるため、`xattr -d com.apple.quarantine` の案内も必要です（[Apple Developer Forums](https://developer.apple.com/forums/thread/706442)）。

## 優先度付きロードマップ（Issue 化用）

AI エージェントと一緒に開発する一人のホビーメンテナを前提に、マイルストーンを依存関係の順に並べました。**v0.3 では Command 層とパスベースのマークを最初に入れます。** ファイル監視、IPC、Herdr への送信がすべてこの上に乗るからです。優先度は P0（リリースを止める）、P1（そのマイルストーンの核）、P2（あると良い）の3段階です。各 Issue はそのまま GitHub に貼れる形にしています。

### マイルストーン v0.2.x — 安全性と CI（目安：数日〜2週間）

#### [P0] 同じディレクトリへの paste でファイルが0バイトになる
- **概要**: `paste()` がコピー元と貼り付け先が同じパスかを確かめずに `fs::copy` するため、ファイルが切り詰められる。ディレクトリでは自分自身や自分の子孫へのコピーで、ツリーが切り詰められるか無限再帰になる。
- **受け入れ基準**: `src == dst` のときは自動で `name (copy)`、`name (copy 2)` … にリネームしてコピーする（または拒否する）。自分の子孫ディレクトリへのコピーはエラーにする。シンボリックリンクは追跡せずリンクとしてコピーする。`tempfile` を使い、自己コピー、子孫へのコピー、シンボリックリンクのループを回帰テストする。
- **根拠**: `src/actions.rs:36-53`、`src/fs_utils.rs:73-85`。macOS で実験し再現を確認済み。両ペインが同じ cwd で起動するため、初回の操作で踏む。

#### [P0] 上書き時の衝突ダイアログ（上書き / スキップ / 両方残す）
- **概要**: paste と rename が既存ファイルを黙って置き換える。
- **受け入れ基準**: 衝突時にファイル名を示したダイアログを出す。「残りすべてに適用」を選べる。rename で既存の名前を指定したら拒否する。テストを付ける。
- **根拠**: `src/actions.rs:47`、`src/input.rs:194-198`（Unix の `fs::rename` は置き換える）。

#### [P0] 削除の安全化：`X` の確認と、確認ダイアログの既定値
- **概要**: `X` は確認なしで完全削除する。「(y/N)」と表示しているのに Enter で削除が実行される。確認ダイアログは件数しか示さない。
- **受け入れ基準**: Enter は「いいえ」として扱い、`y` のときだけ実行する。対象のファイル名を一覧表示する（多い場合は先頭 N 件と残りの件数）。`X` にも確認を出す（ゴミ箱の導入は v0.3 で扱う）。確認ダイアログ中は `q` で終了しない。
- **根拠**: `src/input.rs:21-24, 219-223, 264-280`、`src/ui.rs:137-148`。lf は新規ユーザーを守るため、既定で削除キーを割り当てていない（[lf doc.md](https://github.com/gokcehan/lf/blob/master/doc.md)）。

#### [P0] ステータス/メッセージラインと panic フック
- **概要**: エラーを `eprintln!` で代替画面の裏に出しているか、`let _ =` で捨てている。panic すると端末が raw モードのまま残る。
- **受け入れ基準**: 画面下部にメッセージラインを置き、エラー、成功件数、カーソル位置、選択数を表示する。コード内から `eprintln!` をなくす。panic フックで端末を復元してから、メッセージを表示する。
- **根拠**: `src/actions.rs:45,48,61,65`、`src/main.rs:33-40`。

#### [P1] ディレクトリ移動の失敗で表示と実体がずれる
- **概要**: `on_enter`/`on_left` は `current_dir` を先に書き換えてから refresh し、その失敗を無視する。そのため権限のないディレクトリに入ると、タイトルと一覧が食い違う。
- **受け入れ基準**: refresh が成功したときだけ移動を確定し、失敗はメッセージラインに出す。読めないエントリが一つあっても一覧全体を失敗させない。`h` で親に戻ったら、直前にいたディレクトリにカーソルを戻す。
- **根拠**: `src/app.rs:17,28,93-99,134-136`。

#### [P1] 使っていないマウスキャプチャを無効化する
- **概要**: `EnableMouseCapture` を有効にしているがイベントを処理しないため、端末のテキスト選択だけが失われている。
- **受け入れ基準**: マウス対応を実装するまではキャプチャを無効にし、端末でパスをドラッグして選択できるようにする。
- **根拠**: `src/main.rs:26`、`src/input.rs:19`。

#### [P1] 新しい CI と初回リリースが動くことを確認する
- **概要**: `ci.yml`（fmt/clippy/test を ubuntu-24.04 と macos-latest で実行）と書き直した `release.yml`（darwin ×2、linux-musl ×2、tar.gz + sha256、タグとバージョンの検査、`softprops/action-gh-release@v3`）は作成済みだが、GitHub 上ではまだ動かしていない。
- **受け入れ基準**: 上記の P0 安全修正（paste、上書き、削除）をマージしてからリリースする（データ消失バグ入りのバイナリを新しいパイプラインで最初に配らないため）。タグを打つ前に `Cargo.toml` を 0.2.1 に上げる（release.yml がタグとバージョンの一致を検査するため）。push と PR で CI が通る。`v0.2.1` タグで4ターゲットの成果物と `.sha256` が揃う。ダウンロードした arm64 バイナリが ad-hoc 署名のまま起動する。musl バイナリが WSL2 で起動する。可能ならドラフトリリースで中身を確認してから公開する。
- **根拠**: 作業は進行中。ripgrep と yazi の構成に準拠（[ripgrep release.yml](https://raw.githubusercontent.com/BurntSushi/ripgrep/master/.github/workflows/release.yml)）。strip が署名を壊さないかは未検証なので要確認。

#### [P1] README にインストール手順と対応プラットフォームを書く
- **概要**: README にはバイナリの入手方法が書かれていない。リリース対象から Windows が外れた。ffmpeg の案内が `brew` 前提になっている。
- **受け入れ基準**: 次の内容を README に書く。対応環境は macOS（arm64/x86_64）と Linux/WSL2（musl）で、Windows は対象外または best-effort であること。curl で取得して展開する手順。ブラウザでダウンロードした場合の `xattr -d com.apple.quarantine` の案内。`cargo install --git` での導入。`apt install ffmpeg` の併記。`KURA_IMAGE_PROTOCOL` の説明。
- **根拠**: curl でダウンロードしたファイルには quarantine 属性が付かない（[Apple Developer Forums](https://developer.apple.com/forums/thread/706442)）。`README.md:52`。

#### [P2] Herdr 0.9.x で画像プローブを再テストし、upstream に報告する
- **概要**: Herdr の Kitty 応答の順序問題への回避策は 1798f55 で実装済み。Herdr 0.9.2 で graphics が書き直され、0.9.4 で画像対応の広告の挙動が変わったので、現行版で回避策がまだ必要かを確かめる。
- **受け入れ基準**: Herdr 0.9.5 以降と Ghostty または Kitty の組み合わせで、`KURA_IMAGE_PROTOCOL` を指定しなくても画像が出ることを確認する。まだ再現する場合は、Kitty 仕様の「クエリには即座に応答」に基づいて Herdr（必要なら ratatui-image）に issue を出す。
- **根拠**: [herdr releases](https://github.com/herdrdev/herdr/releases)、[Kitty graphics protocol](https://sw.kovidgoyal.net/kitty/graphics-protocol/)、`src/graphics.rs`。

### マイルストーン v0.3 — ADE 対応の中核（目安：1〜2か月）

#### [P0] Command 層の導入（入力 → Command → `App::apply`）
- **概要**: キー解析、モード遷移、ファイル操作の副作用を分離する。キーマップ設定、IPC、テストのどれからも同じコマンドを使えるようにする。
- **受け入れ基準**: `enum Command`（移動、マーク、yank、paste、削除、rename、send-to-agent など）を定義し、`input.rs` はキーを Command に変換するだけにする。ファイル操作は `plan_paste(src, dst) -> Vec<Op>` のような純粋関数を経由させ、端末なしで単体テストできるようにする。
- **根拠**: 現状は330行の単一ディスパッチャ（`src/input.rs:244-336`）。後に続くすべての Issue の前提になる。

#### [P0] マークとカーソルをパスで保持し、refresh 後も維持する
- **概要**: `HashSet<usize>` を `HashSet<PathBuf>`（あるいは順序付きの集合）に置き換える。refresh やソートの後もカーソルを名前で復元する。ソート設定を保持する。
- **受け入れ基準**: refresh、ソート、paste、外部からのファイル追加の後も、マークと選択が同じファイルを指している。ソート設定が refresh で失われない。ディレクトリを先頭にまとめるオプションを付ける。
- **根拠**: `src/app.rs:12,29-32`、`src/fs_utils.rs:52`。ファイル監視の前提条件。

#### [P1] ゴミ箱を既定にし、完全削除は明示操作にする
- **受け入れ基準**: `x` は `trash` crate で OS のゴミ箱へ移す。完全削除は `X` と確認、または設定で有効化する。macOS では復元一覧を使えないことを README に書く。
- **根拠**: [docs.rs trash](https://docs.rs/trash/latest/trash/)。yazi と superfile はネイティブのゴミ箱を標準搭載している（[superfile v1.6.1](https://github.com/yorukot/superfile/releases/tag/v1.6.1)）。

#### [P1] ファイル監視による自動更新と「最近変わったファイル」の強調表示
- **受け入れ基準**: `notify` と debouncer で表示中の2ディレクトリを監視し、変更があれば再描画する（マークとカーソルは維持）。セッション開始以降または N 秒以内に変わったファイルを色で区別する。WSL2 の `/mnt/c` などに備えて、ポーリングにフォールバックできる設定を用意する。アイドル時は 100ms ごとの再描画をやめ、イベント駆動にする。
- **根拠**: [docs.rs notify](https://docs.rs/notify/latest/notify/)、lf の `watch`（[lf doc.md](https://github.com/gokcehan/lf/blob/master/doc.md)）。「エージェントが何を触ったか」を見るための中核機能。

#### [P1] git status の表示と、変更ファイルのフィルタ
- **受け入れ基準**: `git status --porcelain=v2` をサブプロセスで実行し、各行にステータス記号を出す。ブランチ名をステータスラインに表示する。「変更ファイルだけ表示」の切替を設ける。リポジトリ外では何もしない。
- **根拠**: broot は組み込み、yazi はプラグインで対応している（[broot](https://dystroy.org/broot/)、[yazi-rs/plugins](https://github.com/yazi-rs/plugins)）。Orca のエクスプローラーも git status で色分けしている。

#### [P1] `@path` のコピーとエージェントへの送信（汎用版）
- **受け入れ基準**: 選択またはマークしたファイルを、cwd 基準の相対パスかつ `@` 付き（設定で絶対パスも可）でクリップボードに入れる。手段は OSC 52 を優先し、pbcopy / wl-copy / xclip / clip.exe にフォールバックする。複数ファイルは空白区切りにする。tmux の `set-clipboard` の注意を README に書く。
- **根拠**: Claude Code の `@` は複数のパスを受け付ける（[Claude Code workflows](https://code.claude.com/docs/en/common-workflows)）。[tmux wiki Clipboard](https://github.com/tmux/tmux/wiki/Clipboard)。yazi の agent 系プラグインと同じ水準を最低限そろえる。

#### [P1] 2つの worktree の差分表示（2画面の決め手）
- **受け入れ基準**: 左右のカーソル下のファイル同士の diff を、片方のペインまたは全画面に表示する（`git diff --no-index` を使い、`delta` があれば通す）。単一ペインでは選択ファイルの `git diff` を表示する。
- **根拠**: 調査した範囲では、「main と agent の worktree を並べてワンキーで diff」を提供するツールはなかった。Orca の価値の中心は worktree と diff のレビューにある（[Orca GitHub](https://github.com/stablyai/orca)）。

#### [P2] `$EDITOR` で開く、cd-on-exit、CLI 引数
- **受け入れ基準**: `e` で `$EDITOR` を開き、終了後に TUI を復元する。`kura [LEFT] [RIGHT]` で左右のパスを指定できる。`--help` と `--version` を付ける。`--last-dir-path <file>` と、bash/zsh/fish 用のラッパー関数を用意する。
- **根拠**: lf、nnn、superfile が標準で備えている（[lf doc.md](https://github.com/gokcehan/lf/blob/master/doc.md)、[nnn](https://github.com/jarun/nnn)）。

#### [P2] WSL でファイルを開く処理と ffmpeg の案内
- **受け入れ基準**: `WSL_DISTRO_NAME` か `/proc/sys/kernel/osrelease` で WSL を検出し、`explorer.exe` に `wslpath -w` で変換したパスを渡す。環境変数か設定で opener を上書きできるようにする。失敗はメッセージラインに出す。
- **根拠**: wslu は開発終了（[Ubuntu archive bug](https://lists.snapcraft.io/archives/ubuntu-archive/2025-December/113453.html)）。[MS Learn: WSL interop](https://learn.microsoft.com/en-us/windows/dev-environment/wsl-interop)。

#### [P2] 隠しファイルと .gitignore の切替、検索の改善
- **受け入れ基準**: 隠しファイルの表示切替、`.gitignore` を尊重するモード、検索の `n`/`N`、現在位置から先だけでなく全体を対象にするマッチ。
- **根拠**: `src/ui.rs:202-203`、`src/fs_utils.rs:56-70`。broot の hidden/ignored 切替（[broot](https://dystroy.org/broot/)）。

### マイルストーン v0.4 — ADE 連携（目安：v0.3 の後 1〜2か月）

#### [P1] Herdr プラグインとしての配布
- **受け入れ基準**: `herdr-plugin.toml` を用意する（`[[panes]]` に split と popup の2種類、`[[actions]]` に「kura を開く」）。リポジトリに `herdr-plugin` トピックを付け、`herdr plugin install` で導入できるようにする。README にキーバインド例（`prefix+f`）を書く。`[[events]] on="worktree.created"` で新しい worktree を開くことも検討する。
- **根拠**: [Herdr plugins docs](https://herdr.dev/docs/plugins/)。Herdr には2画面ファイラのプラグインがない（[awesome-herdr](https://github.com/yigitkonur/awesome-herdr/blob/main/catalog/06-worktrees-and-terminal.md)）。popup にはペイン ID がなく Esc を飲み込む点に注意する。

#### [P1] Herdr ソケット経由で隣のエージェントに `@path` を送る
- **受け入れ基準**: `HERDR_ENV=1` のとき、`HERDR_SOCKET_PATH` に NDJSON で接続する（依存は serde_json と UnixStream だけ）。`pane.neighbor` か `agent.list` で送り先を決め、`pane.send_text` で `@path ` を入力する（Enter は押さない）。送り先が複数ある場合は選択 UI を出す。Herdr 外では v0.3 のクリップボード経路にフォールバックする。
- **根拠**: [Herdr socket API](https://herdr.dev/docs/socket-api/)。`herdr-yazi-agent` と同等の機能（[GitHub](https://github.com/timofey-TK/herdr-yazi-agent)）。

#### [P1] worktree の一覧と切替
- **受け入れ基準**: `git worktree list --porcelain`（Herdr 内なら `worktree.list` と `agent.get` の cwd）から選んで、片方のペインで開く。どの worktree をどのエージェントが使っているかを表示する。
- **根拠**: worktree ごとにエージェントを動かす形は Herdr、Orca、Claude Code（`--worktree`）で共通（[Claude Code workflows](https://code.claude.com/docs/en/common-workflows)、[Herdr socket API](https://herdr.dev/docs/socket-api/)）。

#### [P2] 「エージェントが変更したファイル」の仮想ビュー
- **受け入れ基準**: Claude Code の `PostToolUse`（`Edit|Write`）フックが `tool_input.file_path` を追記するログを kura が監視し、片方のペインに一覧として表示する。フック設定の例を README に載せる。Herdr の `pane.agent_status_changed → done` で更新を通知することも検討する。
- **根拠**: [Claude Code hooks](https://code.claude.com/docs/en/hooks)。Herdr のエコシステムでは herdr-shelf などの「エージェントが触ったファイル」ツールに需要がある（[awesome-herdr](https://github.com/yigitkonur/awesome-herdr)）。

#### [P2] 外部から操作する CLI/IPC（`kura --send` / reveal）
- **受け入れ基準**: 実行中の kura に「このパスを表示」「このディレクトリに cd」を送れるソケットと、`kura msg reveal <path>` のような CLI。v0.3 の Command 層をそのまま公開する。
- **根拠**: lf の `-remote`、yazi の `ya emit`、broot の `--send`（[lf doc.md](https://github.com/gokcehan/lf/blob/master/doc.md)、[yazi DDS](https://yazi-rs.github.io/docs/dds)、[broot CHANGELOG](https://github.com/Canop/broot/blob/main/CHANGELOG.md)）。エージェントから「このファイルを見て」と指示できるようになる。

#### [P2] Claude Code IDE プロトコルによる `@mention`（実験的機能）
- **受け入れ基準**: 機能フラグの裏で `~/.claude/ide/<port>.lock` と WebSocket MCP サーバーを立て、選択ファイルを `at_mentioned` で送る。壊れた場合でも kura 本体に影響しない。
- **根拠**: claudecode.nvim と yazi-claude-ide に実装例がある（[claudecode.nvim](https://raw.githubusercontent.com/coder/claudecode.nvim/main/README.md)、[yazi-claude-ide](https://github.com/FunnyQ/yazi-claude-ide)）。非公式プロトコルなので優先度は低い。

### マイルストーン later — 余力があれば

#### [P2] 設定ファイルとキーマップ（TOML）
- **受け入れ基準**: `~/.config/kura/config.toml` でキー、ソート、opener、プレビュー、入力ソースの自動切替（現在は無効にできない）を設定できる。`?` でキーマップのヘルプを出す。
- **根拠**: 現在はキーがハードコードされている（`src/input.rs:244-336`）。入力ソースの切替は終了時に元へ戻さない（`src/input_source.rs`）。norte は7種のキーマッププリセットを持つ（[norte README](https://github.com/compilando/norte)）。

#### [P2] 非同期のファイル操作とプレビュー（進捗表示・キャンセル）
- **受け入れ基準**: 大きなコピー、削除、ffmpeg の実行をワーカースレッドで行い、ステータスラインに進捗を出し、キャンセルできるようにする。毎フレームの stat をキャッシュに置き換える。
- **根拠**: 現在は UI スレッドで同期処理している（`src/media.rs`、`src/ui.rs:199-216`）。yazi は完全非同期で、タスクの進捗とキャンセルを持つ（[yazi GitHub](https://github.com/sxyazi/yazi)）。

#### [P2] MCP サーバー（`kura mcp`）
- **受け入れ基準**: reveal、list_changed、get_selection などの読み取り中心のツールを公開し、書き込み系は扱わない。
- **根拠**: norte がすでに MCP と統制（スコープ、ジャーナル）の領域を押さえている（[norte README](https://github.com/compilando/norte)）。kura は「人間の選択をエージェントに渡す」読み取り側に絞って差別化する。v0.4 の IPC があれば薄いラッパーで済む。

#### [P2] タブ、ブックマーク、zoxide、ファジージャンプ、構文ハイライト、Markdown プレビュー、`$EDITOR` での一括リネーム
- **根拠**: 2025〜2026年のファイラの標準的な機能群（[yazi CHANGELOG](https://github.com/sxyazi/yazi/blob/main/CHANGELOG.md)、[pelorus](https://github.com/mogglemoss/pelorus)）。ADE の主題からは外れるので、需要が見えてから取り組めば十分。

## 結論

調査を通じて見方が変わったのは、**競争の軸がレイアウトではなく「エージェントとの接点の作り方」に移った** という点です。2画面であることは、yazi に対しては依然として差別化になります。しかし同じ2画面の norte や termide は「エージェントにファイルを操作させる」方向へ進んでおり、yazi はプラグインで send-to-agent をすでに満たしています。kura に残っている、しかもまだ誰も取っていない立ち位置は、**人間が判断する側の道具** です。エージェントの変更を worktree をまたいで見比べ、選んで渡し、安全に取り込む。2画面の形がそのまま意味を持つ用途で、作者自身の環境（macOS、Herdr、日本語入力）とも一致しています。

ここから導かれる実務上の帰結は二つあります。一つは、ADE の中で使われる道具の価値は「エージェントが書き換えている最中のファイルを安全に扱えるか」で決まるので、**v0.2.x の安全修正と v0.3 のパスベースのマークは、地味でも差別化の土台そのものだということ**です。もう一つは、Herdr プラグインと `@path` 送信は比較的小さな実装で「Herdr 唯一の2画面ファイラ」という分かりやすい看板を得られるので、v0.3 の Command 層ができ次第、早めに出す価値が高いということです。AI エージェントと開発するホビープロジェクトとして、一つの Issue が一つのエージェントセッションで終わる粒度に切ってあるので、このロードマップはそのまま運用できるはずです。
