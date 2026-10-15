# OPC UA 実装計画

- 更新日: 2026-10-15
- 親課題: [banto-industrial #201](https://github.com/tyaro/banto-industrial/issues/201)
- 対象: 本リポジトリの [#3〜#9](https://github.com/tyaro/banto-opcua/issues)

## 現在地と完了の単位

PR #1 はサーバーの起動と標準ノード `ServerStatus/State` の Read までを実装した。
`namespace_uri` は設定にあるが、タグのアドレス空間、Browse、タグの Read、
Subscription はまだない。ハブとの接続、管理 UI、Windows Service も未実装。
以下は依存関係に沿った作業順であり、各段階を独立した PR で検証する。

| 段階 | 作業 | 完了条件 |
| --- | --- | --- |
| 0. 足場の堅牢化 | [#3](https://github.com/tyaro/banto-opcua/issues/3) 接続失敗の回帰テスト、[#4](https://github.com/tyaro/banto-opcua/issues/4) 設定 API と公開ホストの検証 | 時間切れ・キャンセルでタスクが残らず、不正な公開ホストを起動前に拒否する。Linux / Windows の CI が通る |
| 1. 固定タグの公開 | 本リポジトリに Phase 1 の課題を起票する。固定名前空間と少数の読み取り専用 Variable を登録し、Browse / Read / Subscription を提供する | 同梱クライアントと UaExpert で接続・Browse・Read・Subscribe を確認する。NodeId と namespace URI は README の決定事項に従う |
| 2. ハブの現在値 | [#6](https://github.com/tyaro/banto-opcua/issues/6) 値・品質の写像、[#8](https://github.com/tyaro/banto-opcua/issues/8) 時刻の写像、banto-industrial #201 のタグレジストリ接続 | Good / Bad / Stale、Boolean / Double、時刻不明の null を実データで確認する。設定、再起動、Windows Service、管理 UI は #201 で追跡する |
| 3. 購読と動的ノード | [#7](https://github.com/tyaro/banto-opcua/issues/7) 品質だけの変更、[#5](https://github.com/tyaro/banto-opcua/issues/5) 削除と復帰 | 値が同じでも品質変更が通知される。削除通知後も MonitoredItem が残り、同じ NodeId の復帰で通知が再開する |
| 4. 外部実装 | [#9](https://github.com/tyaro/banto-opcua/issues/9) Python asyncua と opc-plc | プレースホルダーを失敗可能な実テストに替える。バージョンを固定し、nightly と手動実行を緑にする |

## 依存関係と実装上の判断

- 段階 0 は他の段階と独立して進める。`ServerConfig` の拡張方法は、
  `Default` と `with_*` メソッドを使う方式を採る。`#[non_exhaustive]` を
  付けると外部クレートでは `..Default::default()` を含む構造体リテラルが
  使えなくなるため、利用者へ移行方法を記載する。
- #3 の実 TCP テストは鍵生成に時間がかかるため、PR の必須 CI では
  `#[ignore]` とし、Linux / Windows の nightly と手動実行で確認する。
- 段階 1 が #5・#7・#9 のタグ向け受け入れテストの前提。Phase 1 の課題には
  固定ノードの値更新方法と最小限のテスト用クライアント操作も含める。
- #6 は [banto-industrial #615](https://github.com/tyaro/banto-industrial/issues/615)
  で REST / WS / gRPC と共通契約にするかを決める。OPC UA 側は NaN / ±Inf、
  0 / 1 以外の bit を値なし・Bad とし、現行方針どおり Bad の細分化をしない。
  core の純関数はハブと独立して実装できる。
- #8 は [banto-industrial #614](https://github.com/tyaro/banto-industrial/issues/614)
  が共通読み取り層へ値の採取時刻と品質を認識した時刻を追加してから結合する。
  core の写像は Good に採取時刻、Bad / Stale に認識時刻、不明なら null を返す。
  Read のたびに SourceTimestamp を現在時刻へ差し替えない。
- #7 は async-opcua 0.19 のフィルタ未指定時の値だけの比較に阻まれる。
  `notify_data_change` を呼ぶだけでは品質だけの変化は通らない。upstream へ
  再現例を報告し、MonitoredItem の既定フィルタを変える拡張点か upstream の
  修正で解決する。MPL-2.0 のソースは本リポジトリの MIT ファイルへコピーしない。
- #5 はタグの追加・削除を扱う node manager の導入後に実装する。
  `BadNodeIdUnknown` を通知する順序と、削除後も MonitoredItem が残ることを
  実際の Subscription で確かめる。
- #9 の Python 経路は段階 1 の後に先行できる。opc-plc 経路には、現状の
  `ServerStatus/State` 専用 Read より広い、NodeId を指定する最小限の
  クライアント Read API が要る。製品としての OPC UA クライアントは #201 の
  対象外として別課題に残す。外部テストは `ci-gate` の必須項目には入れない。

## 各 PR の検証

Rust を変更する PR は `cargo fmt --all -- --check`、
`cargo clippy --workspace --all-targets -- -D warnings`、`cargo test --workspace` を
Linux と Windows で実行する。依存関係を変えたときは `cargo deny --all-features check`
も実行する。タグと購読の変更は、値・StatusCode・SourceTimestamp を実際の
DataValue として確認するプロセス内テストを追加する。外部実装のテスト手順と
固定バージョンは `interop/README.md` に記録する。

段階 1 の着手時には #201 の説明に残る NodeId や DataType の候補案を
[README の決定事項](../README.md#決定事項) に合わせて更新し、ハブ側の設定・
管理 UI・Windows Service の作業を追跡できる単位へ分ける。
