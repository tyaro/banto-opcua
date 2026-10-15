# banto-opcua の不変条件

対象は、このライブラリを保守する人。banto 本体の画面・認証の規約
（conventions.md の REST / Tauri）はここでは複製しない。言語の体裁は
rustfmt / clippy に委ねる。

## 1. 既定の待受はループバック

サーバーの既定 `bind` は `127.0.0.1`。LAN へ出すのは呼び出し側がアドレスを
明示したときだけ。Phase 1 のセキュリティは `None` / Anonymous なので、
ループバック以外は平文かつ匿名の公開になる。その組み合わせを暗黙の既定にしない。

## 2. 初期スコープは読み取り専用

変数のアクセスレベルは CurrentRead のみ。OPC UA の Write から PLC へ
書く経路は別 issue。このリポジトリに書き込みの入口を足すときは、
banto-hub の認可と監査を経由する設計を先に ADR にする。

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
