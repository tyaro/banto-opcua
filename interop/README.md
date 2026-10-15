# 相互接続テスト（未実装）

PR の CI が回すのは単体テストと、ループバックのプロセス内テストである。
外部実装との確認は [`.github/workflows/interop.yml`](../.github/workflows/interop.yml)
に分離してある。

予定している相手:

- **Python [asyncua](https://github.com/FreeOpcUa/opcua-asyncio)** をクライアントにし、本リポジトリのサーバーを Browse / Read / Subscribe する。
- **[opc-plc](https://hub.docker.com/r/mcr.microsoft.com/iotedge/opc-plc)**（`mcr.microsoft.com/iotedge/opc-plc`）を Linux 上でサーバーにし、`banto-opcua-client` から Read する。

どちらも、タグのアドレス空間とクライアントの読み取り API が足場を超えてから
にする。ワークフローは起動条件だけ先に置いてあり、ジョブは成功する TODO である。

UaExpert による手動確認と、OPC Foundation CTT はさらに後続。README の
「テストの層」を参照。
