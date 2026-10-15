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

このリポジトリは足場である。標準アドレス空間で待受し、プロセス内で
クライアントが `ServerStatus/State` を読むところまで。タグのアドレス空間、
購読、ハブの設定 UI はまだ無い。

## ロードマップ（banto-industrial #201）

| 段階 | 内容 | 置き場 |
| --- | --- | --- |
| 足場（この PR） | クレート分割、基盤の ADR、ループバックのプロセス内疎通 | 本リポジトリ |
| Phase 1 | サーバー起動、固定の名前空間、少数の変数、UaExpert から Browse / Read / Subscribe | 本リポジトリ、その後ハブへ接続 |
| Phase 2 | タグレジストリ、アドレス空間、型・品質・時刻の写像、設定、管理 UI、Windows サービス | 主に banto-hub。写像の純関数は core |
| Phase 3 | 証明書、SecurityPolicy、認証、トラスト、相互接続、ソーク | 本リポジトリとハブ |
| 別 issue | OPC UA クライアント製品、Write、Method、Event、Historical Access | 未起票または別 issue |

#201 の初期スコープ外はクライアント製品、Write、Method、Event / Alarm、
Historical Access、コンパニオン仕様、Discovery Server、細かい権限である。

Phase 1 の試験用セキュリティは `None` / `None` / Anonymous。既定 bind が
ループバックであることを、この組み合わせの条件にする。証明書管理 UI は
Phase 3 まで前倒ししない。

## テストの層

| 層 | いつ | 何を見るか |
| --- | --- | --- |
| 単体 | PR の CI（Linux と Windows） | core の写像。サーバーを起動しない |
| プロセス内 | 同上 | `127.0.0.1` の空きポートでサーバーを起動し、同梱クライアントが接続して `ServerStatus/State` を読む |
| 相互接続 | 別ワークフロー（`workflow_dispatch`、毎日、main への push、PR はパスで絞る） | 予定: Python asyncua クライアント、`mcr.microsoft.com/iotedge/opc-plc`。中身は未実装の TODO |
| 手動 | 開発者の手元 | UaExpert から Browse / Read / Subscribe |
| 後続 | 認証試験が要るとき | OPC Foundation Compliance Test Tool（CTT） |

プロセス内テストは外部プロセスもコンテナも要らない。相互接続は PR の
必須チェックにしない（プレースホルダが失敗で本線を止めないように、別
ワークフローにしてある）。

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

`async-opcua-crypto` が `rsa` 0.9.10 を引き、RUSTSEC-2023-0071（Marvin Attack、タイミングサイドチャネル）が付いている。2026-09-12 時点で修正版は無い。Phase 1 のエンドポイントは SecurityPolicy `None` で、RSA の秘密操作はしない。`deny.toml` はこの advisory を、Phase 3 で RSA 系ポリシーを開くまでの期限付きで無視している。

## オーナー判断待ち

2026-10-11 の #201 レビューと、この足場を置いた時点で残っているもの。

1. **NodeId の永続性。** 外部名（リネームでクライアントの参照が切れる）か、OPC UA 専用の安定 ID か。core の `NodeId` は数値と文字列の両方を入れられるようにしてあり、どちらにも決めていない。
2. **DataType。** レビューの最小案（`bit` → Boolean、収集済み数値 → Double、`string` は値公開しない）を core の暫定関数にしてある。スケーリング後の小数を整数型へキャストする実装にはしない。最終決定は未了。
3. **品質。** Good / Bad / Stale だけを写す。Bad を `BadNoCommunication` とタグ単位エラーに分けない。Stale は `UncertainLastUsableValue`。この対応でよいかは未確認。
4. **時刻。** SourceTimestamp は呼び出し側が渡した採取時刻で、Read のたびに今へ更新しない。ハブの `ptime_ms`（最後の試行時刻）をそのまま SourceTimestamp にしてよいかはハブ側の契約として未決。
5. **削除されたノードの購読。** `BadNodeIdUnknown` の定数だけ置いた。通知の実装は Phase 2。
6. **既定ポート。** ライブラリの既定は 4840。ハブが別ポートにするかはハブの設定で決める。
7. **Phase 1 を None / Anonymous のまま LAN opt-in できるか。** ライブラリは bind を変えれば待受する。平文・匿名でどのタグが見えるかを、ハブの設定画面と運用条件に書く必要がある。
8. **クライアント製品の issue。** クレートの箱だけある。スコープは未起票の別 issue。
