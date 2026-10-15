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

Phase 1 の固定タグを公開する。`Objects/Banto/PLC1/Line1` の `Speed` と
`Running` を Browse / Read / Subscribe できる。ハブのタグレジストリ、
設定 UI はまだ接続していない。

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
| 相互接続 | 別ワークフロー（`workflow_dispatch`、毎日、main への push、PR はパスで絞る） | 予定: Python asyncua クライアント、`mcr.microsoft.com/iotedge/opc-plc`。中身は未実装の TODO |
| 手動 | 開発者の手元 | 下記のサンプルサーバーを起動し、UaExpert から Browse / Read / Subscribe |
| 後続 | 認証試験が要るとき | OPC Foundation Compliance Test Tool（CTT） |

プロセス内テストは外部プロセスもコンテナも要らない。相互接続は PR の
必須チェックにしない（プレースホルダが失敗で本線を止めないように、別
ワークフローにしてある）。

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
3. **品質。** Bad の細分化は後回しにし、現状を維持する。Good は Good、Bad は Bad、Stale は `UncertainLastUsableValue`（0x40900000）。
4. **時刻。** Good の SourceTimestamp は採取時刻、Bad と Stale は品質を認識した時刻、不明なときは null。ハブ側の変更が必要なので、実装は Phase 2 とし、この足場は方針の記載のみ。
5. **ポート。** 既定は 4840。設定で変更できる。
6. **LAN 公開。** 条件付きで許可する。`advertised_host` を用意し、未指定アドレス（`0.0.0.0` / `::`）でそれが未設定なら起動はエラーになる。ループバック以外は明示したときだけ公開し、そのときは警告ログを出す。既定の bind は `127.0.0.1`。
7. **edition。** 2024 のまま。

クライアント製品のスコープは未起票の別 issue（ロードマップの「別 issue」）。

## Phase 2 の検討事項

実装はこの足場に入れない。方針だけを残す。

- **削除されたノードの購読。** `BadNodeIdUnknown` を通知する。MonitoredItem は残し、同じ NodeId が戻ったら購読を再開する。実装は `notify_data_change`。定数 `StatusCode::BAD_NODE_ID_UNKNOWN` だけ置いてある。
- **非有限値と bit。** NaN / Inf、および 0 と 1 以外の bit 値は Bad とする契約にする。現状のハブは NaN を null かつ Good で出す。
- **品質だけの変化。** 値が変わらず品質だけ Good から Stale になったとき、通知が飛ばない。`DataChangeFilter` はクライアントが指定するもので、サーバーから trigger を強制できない。サーバー側でフィルタ未指定を `StatusValue` として扱う回避策、または upstream（async-opcua）の修正が必要。
