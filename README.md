# banto-opcua

banto ファミリーの汎用 OPC UA ライブラリ。サーバーとクライアントの両方を
置く。最初の利用者は [banto-hub の OPC UA サーバー](https://github.com/tyaro/banto-industrial/issues/201)
（読み取り専用。Browse、Read、Subscription / MonitoredItem、StatusCode と
Timestamp の写像）。

SLMP を外部クレート [tyaro/slmp](https://github.com/tyaro/slmp) に分けたのと
同じ考えで、OPC UA のプロトコル実装は本リポジトリに切り出す。ハブ本体へ
async-opcua を直接依存させない。

- ライセンス: [MIT](LICENSE)（2026-10-15 オーナー決定。単一ライセンス）
- 基盤実装: [ADR-0001](docs/adr/0001-async-opcua-base.md)（async-opcua 0.19、MPL-2.0）
- 不変条件: [docs/conventions.md](docs/conventions.md)
- 変更履歴: [CHANGELOG.md](CHANGELOG.md)

依存の async-opcua は MPL-2.0（ファイル単位のコピーレフト）である。
本クレートのソースは MIT のまま、MPL のコードは Cargo 依存としてリンクする。
依存木全体が MIT という意味ではない。詳細は ADR-0001。

## 構成

```
crates/
  banto-opcua-core/    共有語彙（値、StatusCode、時刻、ノード）。プロトコル非依存
  banto-opcua-server/  サーバー。既定 bind は 127.0.0.1。初期スコープは読み取り専用
  banto-opcua-client/  クライアントの足場。製品機能は別 issue
```

`banto-opcua-core` は async-opcua に依存しない。サーバーとクライアントが
境界で core の型と async-opcua の型を変換する。

Phase 1 の固定タグを既定で公開する。`Objects/Banto/PLC1/Line1` の `Speed` と
`Running` を Browse / Read / Subscribe できる。`ServerConfig::with_sample_tags(false)`
で試験用タグを外し、同じ外部名の実タグを登録できる。
`RunningServer::add_tag_reading` / `update_tag_reading` は型・値・品質・時刻を
`TagReading` で受け取り、読み取り専用の Read / Subscription へ反映する。
未取得でも DataType を保持し、SourceTimestamp が不明なら null のままにする。
ドットを含む名前も Browse でき、Variable NodeId は外部名のまま保持する。
Read と 0 ms / 非ゼロの sampling interval の購読で、入力した SourceTimestamp を
保持する。サーバーの最小値を 0 ms に設定し、購読作成応答でも要求した間隔を確認する。
遅延したサンプルの時刻を保持する async-opcua の修正は、MPL-2.0 の
[別リポジトリ](https://github.com/tyaro/async-opcua/pull/1)で公開している。
この境界の理由は [ADR-0002](docs/adr/0002-dynamic-reading-boundary.md) に記す。
ハブのタグレジストリ、設定 UI はまだ接続していない。

## ロードマップ（banto-industrial #201）

Issue 間の依存関係と実装順は [実装計画](docs/implementation-plan.md) に記す。

| 段階 | 内容 | 置き場 |
| --- | --- | --- |
| 足場 | クレート分割、基盤の ADR、ループバックのプロセス内疎通 | 本リポジトリ |
| Phase 1 | サーバー起動、固定の名前空間、少数の変数、UaExpert から Browse / Read / Subscribe | 本リポジトリ、その後ハブへ接続 |
| Phase 2 | タグレジストリ、アドレス空間、型・品質・時刻の写像、設定、管理 UI、Windows サービス | 主に banto-hub。写像の純関数は core |
| Phase 3 | 証明書、SecurityPolicy、認証、トラスト、相互接続、ソーク | 本リポジトリとハブ |
| 別 issue | OPC UA クライアント製品、Write、Method、Event、Historical Access | 未起票または別 issue |

#201 の初期スコープ外はクライアント製品、Write、Method、Event / Alarm、
Historical Access、コンパニオン仕様、Discovery Server、細かい権限である。

Phase 1 の試験用セキュリティは `None` / `None` / Anonymous。既定 bind は
ループバック、既定ポートは 4840。ループバック以外は明示設定のときだけ、
警告ログ付きで許可する（「決定事項」）。証明書管理 UI は Phase 3 まで前倒ししない。

## テストの層

| 層 | いつ | 何を見るか |
| --- | --- | --- |
| 単体 | PR の CI（Linux と Windows） | core の写像。サーバーを起動しない |
| プロセス内 | 同上 | ループバックで `ServerStatus/State` を読む。固定タグは Browse / Read / DataChange 通知、読み取り専用の AccessLevel を確認する。IPv6 が使える環境では `[::1]` でも標準ノードの疎通を見る |
| 接続失敗 | 接続処理を変更する PR・毎日・手動（Linux と Windows） | Discovery だけを通すプロキシでセッションを止め、時間切れとキャンセルで TCP 接続が閉じることを確認する。PR の必須 `ci-gate` には含めない |
| 相互接続 | 別ワークフロー（`workflow_dispatch`、毎日、main への push、PR はパスで絞る） | Python asyncua → 本サーバーの GetEndpoints / Browse / Read / Subscription と、本クライアント → opc-plc のタグ Read。固定バージョンと実行方法は [`interop/README.md`](interop/README.md) |
| 手動 | 開発者の手元 | 下記のサンプルサーバーを起動し、UaExpert から Browse / Read / Subscribe |
| 後続 | 認証試験が要るとき | OPC Foundation Compliance Test Tool（CTT） |

プロセス内テストは外部プロセスもコンテナも要らない。相互接続は PR の
必須チェックにしない。別ワークフローの失敗はそのジョブを失敗させる。

接続失敗の再現テストは `cargo test -p banto-opcua-server --test client_connect_failure -- --ignored`
で手動実行できる。毎日の実行は
[`client-connect-regression.yml`](.github/workflows/client-connect-regression.yml)。
テストは Linux / Windows の `127.0.0.0/8` を使う。macOS では既定の
ループバック設定だと `127.0.0.2` へ bind できない。

## Phase 1 の固定タグ

`ServerConfig::namespace_uri`（既定 `urn:banto:opcua:tags`）に次を登録する。
名前空間インデックスはサーバーが割り当てるので、固定値に依存せず
`NamespaceArray` を参照する。初期値は試験用であり、ハブの実データではない。

| Browse パス | NodeId の文字列識別子 | DataType | 初期値 |
| --- | --- | --- | --- |
| `Objects/Banto/PLC1/Line1/Speed` | `PLC1.Line1.Speed` | Double | `0.0` |
| `Objects/Banto/PLC1/Line1/Running` | `PLC1.Line1.Running` | Boolean | `false` |

どちらも `AccessLevel` / `UserAccessLevel` は CurrentRead のみ。値の更新は
試験用の `RunningServer::set_fixed_speed` / `set_fixed_running` を使う。
ハブとの結合と品質・時刻の写像は Phase 2。

UaExpert で手動確認するときは `cargo run -p banto-opcua-server --example fixed_tags`
を実行し、表示されたエンドポイントへ SecurityPolicy `None`、Anonymous で
接続する。`Objects/Banto/PLC1/Line1` を開き、2 つの変数を Data Access View
へ追加する。サンプルサーバーの端末で Enter を押すたびに値が変わり、
MonitoredItem に DataChange が届く。`q` で停止する。

## 開発

ツールチェインは `rust-toolchain.toml` の 1.98.1（banto-industrial と同じピン）。

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo deny --all-features check
```

`cargo deny` は [deny.toml](deny.toml)。ライセンスの許可リストは実測した
依存木だけを載せる。

## 配布

banto の ADR-0011 に合わせ、配布は git タグを予定する。crates.io への公開は
未決のため `publish = false`。消費側は `Cargo.lock` の再現性を保つため、
タグを固定して依存する（banto-industrial の `slmp` と同じ）。

## 既知の依存の注意

`rsa` 0.9.10（`async-opcua-crypto` 経由）の RUSTSEC-2023-0071 を、見直し期日付きで `deny.toml` が無視している。理由と期限は [ADR-0001](docs/adr/0001-async-opcua-base.md)。

## 決定事項

2026-10-15 のオーナー決定。

1. **NodeId。** 外部名 `{connection}.{group}.{tag}` を使う。`tag_id` は削除して作り直すと変わるため使わない。2026-09-30 の banto-industrial のオーナー決定に準拠する。core の `NodeId::external_name` がこの形を作る。数値 NodeId は標準ノード用に残す。
2. **DataType。** 収集済みの数値は Double に固定する。`bit` は Boolean、`string` は周期収集の対象外なので値を公開しない。Int16 や Float には分けない。
3. **品質と値。** Bad の細分化は後回しにする。有効な値では Good は Good、Stale は `UncertainLastUsableValue`（0x40900000）として最後の値を保持する。Bad 品質や欠損値は値なし・汎用 `Bad`（0x80000000）。Double の NaN / ±Inf、Boolean に写す bit の 0 / 1 以外も値なし・汎用 `Bad` とし、丸めない。Stale でも値が無効なら Bad を優先する。bit の `-0.0` は `0.0` と同じ false として扱う。core の `map_current_value` がこの契約を実装する。REST / WS / gRPC と共通化するかは [banto-industrial #615](https://github.com/tyaro/banto-industrial/issues/615) で検討中。
4. **時刻。** Good の SourceTimestamp は採取時刻、Bad と Stale は品質を認識した時刻（または実際に復旧を試みた時刻）、不明なときは null。別の時刻や Read 時の現在時刻では補わない。core の `SampleTimestamps::for_quality` に写像を実装した。引数の品質はハブの入力品質であり、Good の無効値が Bad に写った場合は採取時刻を異常の観測時刻とみなす。固定タグの既存経路では、反復 Read でも保存済み SourceTimestamp が変わらないことを確認した。ハブの実データへの接続には [banto-industrial #614](https://github.com/tyaro/banto-industrial/issues/614) の時刻提供が必要。
5. **ポート。** 既定は 4840。設定で変更できる。
6. **LAN 公開。** 条件付きで許可する。`advertised_host` を用意し、未指定アドレス（`0.0.0.0` / `::`）でそれが未設定なら起動はエラーになる。ループバック以外は明示したときだけ公開し、そのときは警告ログを出す。既定の bind は `127.0.0.1`。
7. **edition。** 2024 のまま。

クライアント製品のスコープは未起票の別 issue（ロードマップの「別 issue」）。

## Phase 2 の検討事項

ハブとの結合に必要な残件を記録する。

- **削除されたノードの購読。** タグを削除すると `BadNodeIdUnknown` を通知する。MonitoredItem を残し、同じ NodeId が戻ったら購読を再開する。リネームは旧ノードの削除と新ノードの追加で表す。実 Subscription テストで検証済み。ハブのタグレジストリとの結合は後続。
- **非有限値と bit。** core の値・品質変換をハブの現在値へ接続する。Hub の共通読み取りは [banto-industrial #615](https://github.com/tyaro/banto-industrial/issues/615) で値なし・Bad に統一された。
- **品質だけの変化。** [async-opcua PR #274](https://github.com/FreeOpcUa/async-opcua/pull/274) の修正済みコミットを固定し、フィルタ未指定でも Good → Stale → Good の通知を実 Subscription テストで確認した。Good → Bad では core の契約どおり値を Null にして通知し、復帰も確認した。公開版への依存復帰と Null Variant 回避策の見直しは [#17](https://github.com/tyaro/banto-opcua/issues/17) で追跡する。ハブの実タグとの結合は後続。
