# banto-opcua の不変条件

対象は、このライブラリを保守する人。banto 本体の画面・認証の規約
（conventions.md の REST / Tauri）はここでは複製しない。言語の体裁は
rustfmt / clippy に委ねる。

## 1. 既定の待受はループバック

サーバーの既定 `bind` は `127.0.0.1`。ポートの既定は 4840（設定で変更できる）。

`0.0.0.0` や `::` のような未指定アドレスで待受するときは `advertised_host` が
必須で、未設定なら起動はエラーになる。エンドポイント URL と discovery URL には
未指定アドレスを載せない。IPv6 のホストは `[..]` で囲む。

ループバック以外への公開は、bind または `advertised_host` を明示したときだけ
行う。そのときは警告ログを出す。Phase 1 のセキュリティは `None` / Anonymous
なので、ループバック以外は平文かつ匿名の公開になる。

## 2. 書き込みは利用者の書き込み経路を通す

OPC UA の Write は、利用者が渡す書き込みハンドラーを通してだけ受け付ける。
サーバーが自分でノードの値を書き換えたり、PLC へ書いたりしない。ハンドラーが
無いときと、書けないタグの AccessLevel は CurrentRead のみ。認可と監査は
ハンドラーの側（banto-hub の書き込み経路）で行う。設計は
[ADR-0004](adr/0004-write-through-host-handler.md)。

## 3. core はプロトコル実装を知らない

`banto-opcua-core` は async-opcua に依存しない。StatusCode やノードの公開型は
core の型にする。MPL-2.0 の型を MIT の公開 API に載せない
（[ADR-0001](adr/0001-async-opcua-base.md)）。

## 4. MPL のソースを MIT のファイルへコピーしない

async-opcua は依存として使う。ソースの複製や、MPL ファイルの改変を
このリポジトリの MIT ファイルに混ぜない。

## 5. テストの層を混ぜない

単体テストと、ループバックのプロセス内テストは PR の CI で毎回実行する。
外部実装（Python asyncua、`mcr.microsoft.com/iotedge/opc-plc`）との相互接続は
別ワークフロー。UaExpert は手動。OPC Foundation CTT は後続。
詳細は README の「テストの層」。
