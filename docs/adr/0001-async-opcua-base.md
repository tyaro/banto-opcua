# ADR-0001: OPC UA の基盤実装に async-opcua を採用する

- 状態: Accepted
- 日付: 2026-10-15
- 関連: tyaro/banto-industrial#201、banto [ADR-0002](https://github.com/tyaro/banto/blob/main/docs/adr/0002-minimal-dependencies.md)（依存は総保守コストで判断する）、オーナー決定 2026-10-15（本リポジトリのライセンスは MIT の単一ライセンス）、SLMP を外部クレート `tyaro/slmp` に分けた先例

## コンテキスト

banto-opcua は banto ファミリーの汎用 OPC UA ライブラリで、サーバーと
クライアントの両方を置く。最初の利用者は banto-hub の読み取り専用サーバー
（Browse、Read、Subscription / MonitoredItem、StatusCode と Timestamp の
写像。既定 bind は `127.0.0.1`。証明書・ユーザー認証は Phase 3。クライアント
製品は別 issue）。

OPC UA は Part 4（サービス）と Part 6（マッピング）だけで実装量が大きく、
セキュアチャネルとセキュリティポリシーは暗号の境界である。banto の
ADR-0002 は「依存を足さない」を既定にしつつ、自前実装が肥大化する、
セキュリティ境界である、仕様の端が多い、実装が成熟している、の複数に
当てはまるときは依存を採る。SLMP はフレーミングを外部クレート `slmp` に
任せ、banto 側は計画とエラーの写像だけを持った。OPC UA も同じ分け方を
する。

本クレートのライセンスは MIT である（2026-10-15 オーナー決定）。基盤に
する依存のライセンスが MIT と両立するかを、採否の条件にする。

## 決定

**プロトコル実装の基盤に [async-opcua](https://github.com/FreeOpcUa/async-opcua) 0.19（MPL-2.0）を採用する。**

フィルタ未指定の購読で品質変更を通知する [upstream PR #274](https://github.com/FreeOpcUa/async-opcua/pull/274)
は 0.19.0 公開後にマージされた。#7 の実 Subscription 試験を通すため、修正を含む
`1ba60ed14177a2590ee75526af40602987a14123` を基点とする。
2026-10-16、非ゼロの sampling interval で採取時刻を失う不具合を直した
[MPL-2.0 の fork](https://github.com/tyaro/async-opcua/pull/1) の
`60c4f9d72218d78177e59c70a03db7fddcf9d857` を一時的に固定した。
このコミットは fork の保全用タグ `banto-source-timestamp-20261016` でも保持する。
sampling interval が -1 のときは実際の購読周期まで最新値だけを保留する。
両修正を含む公開版が出たら crates.io 依存へ戻す（[#17](https://github.com/tyaro/banto-opcua/issues/17)）。
基点の upstream コミットには 0.19.0 以降の別の変更も含まれるため、依存更新時は
回帰テストと cargo-deny を実行する。

banto-industrial の `deny.toml` は `unknown-git = "deny"` のため、fork の
`https://github.com/tyaro/async-opcua` を一時的に `allow-git` へ追加して
cargo-deny を通す。`required-git-spec = "tag"` は最低要件であり、より厳しい
`rev` 固定も満たすため維持する。追加した許可は #17 の公開版への移行後に削除する。

- `banto-opcua-server` は feature `server`（標準アドレス空間を含む）だけを有効にする。
- `banto-opcua-client` は feature `client` だけを有効にする。
- `banto-opcua-core` は async-opcua に依存しない。値・StatusCode・時刻・ノードの語彙は MIT の型として置き、境界で変換する。外部の型を公開 API に漏らさない。
- 本クレートの `license` は `"MIT"` のままにする。MPL-2.0 へは切り替えない。
- Phase 1 の待受は SecurityPolicy `None` / MessageSecurityMode `None` / Anonymous のみ。既定 bind は `127.0.0.1`。Basic128Rsa15 と Basic256 は OPC Foundation が非推奨にしており、Phase 3 でも既定では開かない。

### ライセンス互換（MIT のライブラリから MPL-2.0 を依存として使う）

async-opcua およびその分割クレート（`async-opcua-server` など）は MPL-2.0。
MPL-2.0 はファイル単位の弱いコピーレフトで、GPL / AGPL / LGPL のような
プロジェクト単位のコピーレフトではない。

- MPL-2.0 第 3.3 節（Larger Work）により、MPL のファイルを含むより大きい
  作品を、それ以外のファイルについては別のライセンスで配布できる。MIT の
  ソースから Cargo 依存として MPL のクレートをリンクすることは、この形に
  当たる。本クレートのファイルを MPL にする義務は生まれない。
- 義務が残るのは MPL のファイル自身である。改変した MPL のファイルは
  MPL-2.0 のまま配布し、受信者がそのソースを得られるようにする。改変せず
  crates.io の版を依存するだけなら、ソースは crates.io と upstream の
  リポジトリが既に配布している。
- MPL のソースを本リポジトリの MIT ファイルへコピーしない。コピーした
  ファイルは MIT にできない。パッチを当てる場合は、その差分を MPL のまま
  別ファイルとして保ち、本クレートの `license = "MIT"` と矛盾させない。
- async-opcua 0.19 のライセンス表示は Exhibit B（Secondary Licenses と
  非互換、という但し書き）を付けていない通常の MPL-2.0 である。Secondary
  License（GPL との組み合わせ）の条項は、本クレートを GPL にしないので
  使わない。
- SPDX の `license = "MIT"` は**このクレートのコード**のライセンスであり、
  依存木全体が MIT であるという意味ではない。MPL を禁止する利用側は、
  本クレートが MIT でも依存として取れない。
- `cargo-deny` は MPL-2.0 を、この依存のために明示的に許可する。
  GPL / LGPL / AGPL は許可しない。`MIT OR Apache-2.0 OR LGPL-2.1-or-later`
  のような OR 式は、MIT か Apache-2.0 が許可されていれば LGPL を許可リスト
  に入れずに充足する。
- 下流の banto-industrial は、ハブのバイナリに MPL のオブジェクトが
  入ることを承知する。同リポジトリの `deny.toml` は CSS パーサ由来で
  MPL-2.0 を既に許可しているので、ライセンスの種類は増えない。ただし
  実行ファイルに載るコピーレフトになる点は、ビルドツールだけの MPL より
  配布時の説明が要る。バイナリ配布者は、改変していない MPL 部分について
  ロックファイルが指す crates.io のソースを示せるようにする。

open62541 とその Rust バインディングも MPL-2.0 である。ライセンスの種類
だけでは案 B を選んでも MIT だけの依存木にはならない。MIT だけにしたい
場合の唯一の道は自前実装（案 C）で、それは退ける。

## 検討した代替案

- **案A（採用）: async-opcua 0.19。** FreeOpcUa 配下の純 Rust 実装。旧
  `locka99/opcua` の後継で、2026-02 に 0.18.0、2026-07-18 に 0.19.0 が
  出ている。クライアントとサーバーの両方があり、tokio 上で動く。
  セキュリティポリシーは `None` に加え `Basic128Rsa15`、`Basic256`、
  `Basic256Sha256`、`Aes128Sha256RsaOaep`、`Aes256Sha256RsaPss` を実装している
  （upstream の結合テストがエンドポイントとして列挙している）。後の 2 つと
  `Basic256Sha256` が Phase 3 の候補になる。Windows は C コンパイラも
  CMake も要らない。欠点は MPL-2.0 であること（上記）と、生成された標準
  アドレス空間が大きく、ビルドが重いこと。
- **案B（不採用）: open62541 の Rust バインディング。** C ライブラリ
  open62541 自体は OPC UA のオープン実装として保守が最も厚い。ライセンスは
  こちらも MPL-2.0。安全な Rust クレート `open62541` の最新リリースは
  調査時点で 0.10.1（2025-10-29）で、下位の `open62541-sys`（0.6.3、
  2026-09-09）より API 面の追従が遅れている。ビルドは同梱 C ソースを
  CMake でコンパイルし、Windows では `iphlpapi` のリンクと、暗号を有効に
  する場合は mbedTLS か OpenSSL が要る。実行モデルは C のイベントループで、
  banto-hub の tokio / axum とは橋が要る。banto の CI（Linux と Windows で
  追加のシステム依存なしに `cargo test`）とも相性が悪い。ライセンス上の
  利点が無く、tokio と Windows の CI で負けるので退ける。
- **案C（不採用）: 自前実装。** 本クレートを依存も含めて MIT だけにできる
  唯一の案。OPC UA のサービスとバイナリ符号化、特にセキュアチャネルを
  自前で持つと、脆弱性の追従が banto 側の負担になる。ADR-0002 の
  「セキュリティ境界では成熟した実装を採る」に反する。SLMP でフレーミングを
  自前にしなかった判断とも揃わない。仕様の端（拡張オブジェクト、
  サブスクリプションのキュー、チャンク）を最初の読み取り専用サーバーのため
  だけに抱える総コストが大きいので退ける。

## 帰結

- サーバーとクライアントは async-opcua 0.19 を workspace 依存にし、feature を分けて有効にする。core には入れない。
- MPL のソースをこのリポジトリへコピーしない。upstream を直すときは upstream へ返すか、MPL のままのパッチとして分離する。
- `deny.toml` の許可ライセンスは、実測した依存木に現れたものだけにする。MPL-2.0 はその実測に含まれる。新しいコピーレフトを足すときはこの ADR を見直す。
- Phase 3 でセキュリティポリシーを開くときは、非推奨の Basic128Rsa15 と Basic256 を既定にしない。
- `async-opcua-crypto` が依存する `rsa` 0.9.10 には RUSTSEC-2023-0071（Marvin Attack。RSA 秘密操作のタイミングサイドチャネル）がある。advisory は 2026-09-12 時点で修正版が無いと書いている。Phase 1 のエンドポイントは SecurityPolicy `None` で、RSA の秘密操作はしない。`deny.toml` はこの advisory を無視する。見直し期日は 2027-01-15、または Phase 3（証明書と SecurityPolicy）に着手するときのいずれか早い方。無視を外せる版が出たら依存を上げ、`deny.toml` の ignore とこの項目を消す。README と `deny.toml` は理由を繰り返さず、この項目を参照する。
- banto-industrial が git 依存で本クレートを取るときは、ハブのバイナリに MPL のコードが含まれることをリリースノートか依存の説明に残す。
- タグのアドレス空間はこの ADR の範囲外。NodeId、DataType、品質、時刻、ポート、LAN 公開は 2026-10-15 に決定し、README の「決定事項」に記載した。
