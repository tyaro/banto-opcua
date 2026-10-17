# OPC UA 実装計画

- 更新日: 2026-10-17
- 親課題: [banto-industrial #201](https://github.com/tyaro/banto-industrial/issues/201)
- 対象: 本リポジトリの [#3〜#9](https://github.com/tyaro/banto-opcua/issues) と [#25](https://github.com/tyaro/banto-opcua/issues/25)

## 現在地と完了の単位

PR #1 はサーバーの起動と標準ノード `ServerStatus/State` の Read までを実装した。
段階 0 は PR #10、段階 1 の固定タグは PR #12 で完了した。段階 2 の
[#6](https://github.com/tyaro/banto-opcua/issues/6) の core 写像は PR #13、
[#8](https://github.com/tyaro/banto-opcua/issues/8) の core 写像は PR #14 で完了。
段階 4 の外部テストも PR #15 で実装し、main の 2 経路が成功した。
[#7](https://github.com/tyaro/banto-opcua/issues/7) は upstream の修正を固定し、
フィルタ未指定の実 Subscription で品質変更を検証した。
[#5](https://github.com/tyaro/banto-opcua/issues/5) のタグ追加・削除と
削除後の購読継続も本リポジトリで実装した。
動的タグの実データ向け登録・更新 API と、サンプルタグを外す設定を追加した（PR #20）。
ハブとの接続と管理 API は banto-industrial #627、外部クライアントによる
ハブの確認は banto-industrial #632 で実装した。Write は ADR-0004 で設計し、
#25 の PR #27（node manager の置き換え）、PR #28（書き込みハンドラー）、
PR #29（Python asyncua の相互接続）で実装した。Hub 側の結合は
banto-industrial #653。Windows Service と UaExpert の実機確認（書き込みを含む）、
Phase 3 のセキュリティは未実施。
以下は依存関係に沿った作業順であり、各段階を独立した PR で検証する。

| 段階 | 作業 | 完了条件 |
| --- | --- | --- |
| 0. 足場の堅牢化 | [#3](https://github.com/tyaro/banto-opcua/issues/3) 接続失敗の回帰テスト、[#4](https://github.com/tyaro/banto-opcua/issues/4) 設定 API と公開ホストの検証 | 時間切れ・キャンセルでタスクが残らず、不正な公開ホストを起動前に拒否する。Linux / Windows の CI が通る |
| 1. 固定タグの公開 | [#11](https://github.com/tyaro/banto-opcua/issues/11) 固定名前空間と少数の読み取り専用 Variable を登録し、Browse / Read / Subscription を提供する | 実接続テストと UaExpert で Browse・Read・Subscribe を確認する。NodeId と namespace URI は README の決定事項に従う |
| 2. ハブの現在値 | [#6](https://github.com/tyaro/banto-opcua/issues/6) 値・品質の写像、[#8](https://github.com/tyaro/banto-opcua/issues/8) 時刻の写像、banto-industrial #201 のタグレジストリ接続 | Good / Bad / Stale、Boolean / Double、時刻不明の null を実データで確認する。設定、再起動、Windows Service、管理 UI は #201 で追跡する |
| 3. 購読と動的ノード | [#7](https://github.com/tyaro/banto-opcua/issues/7) 品質だけの変更、[#5](https://github.com/tyaro/banto-opcua/issues/5) 削除と復帰 | 値が同じでも品質変更が通知される。削除通知後も MonitoredItem が残り、同じ NodeId の復帰で通知が再開する |
| 4. 外部実装 | [#9](https://github.com/tyaro/banto-opcua/issues/9) Python asyncua と opc-plc | プレースホルダーを失敗可能な実テストに替える。バージョンを固定し、nightly と手動実行を緑にする |
| 5. 書き込み | [#25](https://github.com/tyaro/banto-opcua/issues/25) 利用者の書き込みハンドラーに通す Write（[ADR-0004](adr/0004-write-through-host-handler.md)）。Hub 側は banto-industrial #653 | ハンドラーの結果が StatusCode の表どおりに返る。Write だけではノードの値が変わらない。Python asyncua からの Write が nightly で緑 |
| 6. クライアント | [#33](https://github.com/tyaro/banto-opcua/issues/33) Hub のドライバ向けの Read / Write の部品（[ADR-0005](adr/0005-client-as-hub-driver.md)）。ドライバ本体は banto-industrial #678 | まとめて Read / Write がノードごとの結果を返し、切断は接続ごとの失敗になる。opc-plc 相手の相互接続が nightly で緑 |

## 依存関係と実装上の判断

- 段階 0 は他の段階と独立して進める。`ServerConfig` の拡張方法は、
  `Default` と `with_*` メソッドを使う方式を採る。`#[non_exhaustive]` を
  付けると外部クレートでは `..Default::default()` を含む構造体リテラルが
  使えなくなるため、利用者へ移行方法を記載する。
- #3 の実 TCP テストは鍵生成に時間がかかるため、PR の必須 CI では
  `#[ignore]` とし、接続処理を変更する PR の非必須チェックと、Linux / Windows
  の nightly・手動実行で確認する。
- 段階 1 が #5・#7・#9 のタグ向け受け入れテストの前提。Phase 1 の課題には
  固定ノードの値更新方法と最小限のテスト用クライアント操作も含める。
- #6 は [banto-industrial #615](https://github.com/tyaro/banto-industrial/issues/615)
  で REST / WS / gRPC と共通契約にするかを決める。OPC UA 側は NaN / ±Inf、
  0 / 1 以外の bit を値なし・Bad とし、現行方針どおり Bad の細分化をしない。
  core の純関数はハブと独立して実装できる。
- #8 は [banto-industrial #614](https://github.com/tyaro/banto-industrial/issues/614)
  が共通読み取り層へ値の採取時刻と品質を認識した時刻を追加してから結合する。
  core の写像は Good に採取時刻、Bad / Stale に認識時刻、不明なら null を返す。
  Read のたびに SourceTimestamp を現在時刻へ差し替えない。PLC タグと Hub ソースの
  Stale の違いは core に渡す品質認識時刻に表れ、#614 の結合時に実データで検証する。
- #7 は async-opcua 0.19.0 のフィルタ未指定時の値だけの比較に阻まれるが、
  [upstream PR #274](https://github.com/FreeOpcUa/async-opcua/pull/274) で修正済み。
  公開版へ入るまでは修正を含むコミットを固定して使い、公開版が出たら戻す。
  値なし・Bad は Null Variant として渡す。現行 upstream の Variable 読み出しは
  DataValue の Value フィールド自体を省くと StatusCode も落とすため。
  公開版への復帰と Null Variant 回避策の見直しは
  [#17](https://github.com/tyaro/banto-opcua/issues/17) で追跡する。
  banto-industrial は `unknown-git = "deny"` で、`allow-git` に async-opcua の
  URL がない。ハブ結合用のタグは #17 完了後に切るか、先に取り込む場合は
  industrial 側の `allow-git` に同 URL を一時的に加える。
  `required-git-spec = "tag"` は最低要件であり、本依存の `rev` はこれを満たす。
  MPL-2.0 のソースは本リポジトリの MIT ファイルへコピーしない。
- #5 はタグの追加・削除を node manager に実装した。削除通知
  `BadNodeIdUnknown` を流してからノードを外し、MonitoredItem は消さない。
  同じ NodeId の再登録と、リネーム後の新しい NodeId を実 Subscription と
  Browse / Read で確かめる。追加 API の初期値は試験用に登録時刻を付ける。
  空になった接続・グループのフォルダーは、同じ階層への再登録に備えて残す。
  ハブのタグレジストリと実データの時刻との結合は #201 で行う。
- #201 の結合前提として、動的タグの型・品質・時刻を渡す
  `TagReading` と登録・更新 API を用意した（[ADR-0002](adr/0002-dynamic-reading-boundary.md)）。
  ハブはサンプルタグを無効にしてから実タグを登録する。レジストリの照合、
  Stale を含む現在値の反映、設定・管理 UI・Windows Service はハブ側で行う。
- #9 の Python 経路は段階 1 の後に先行できる。opc-plc 経路には、現状の
  `ServerStatus/State` 専用 Read より広い、NodeId を指定する最小限の
  クライアント Read API が要る。#9 の外部テストで UInt32 タグの Read API を
  追加して 2 経路を検証する。製品としての OPC UA クライアントは #201 の
  対象外として別課題に残す。外部テストは `ci-gate` の必須項目には入れない。

## 各 PR の検証

Rust を変更する PR は `cargo fmt --all -- --check`、
`cargo clippy --workspace --all-targets -- -D warnings`、`cargo test --workspace` を
Linux と Windows で実行する。依存関係を変えたときは `cargo deny --all-features check`
も実行する。タグと購読の変更は、値・StatusCode・SourceTimestamp を実際の
DataValue として確認するプロセス内テストを追加する。外部実装のテスト手順と
固定バージョンは `interop/README.md` に記録する。

段階 1 では #201 の説明に残る NodeId や DataType の候補案を
[README の決定事項](../README.md#決定事項) に合わせて更新し、ハブ側の設定・
管理 UI・Windows Service の作業を追跡できる単位へ分ける。
