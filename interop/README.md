# 外部実装との相互接続

`.github/workflows/interop.yml` は Linux で 2 経路を実際に動かし、失敗すれば
ジョブを失敗させる。手動、毎日、main への push、関連パスの PR で実行する。
`ci-gate` の必須チェックには含めない。

| 経路 | 固定バージョン | 確認内容 |
| --- | --- | --- |
| Python asyncua → banto-opcua-server | Python 3.12、`asyncua==1.1.8` と `requirements.txt` の依存 | GetEndpoints、固定タグの Browse・Read・Subscription、更新後の値と Good 品質 |
| banto-opcua-client → opc-plc | `mcr.microsoft.com/iotedge/opc-plc:2.9.9-linux-amd64`（digest `sha256:3d16f104393869540403390e617a2d6afe98b3b0673fb9d8ff1533f35b6726cf`） | `ServerStatus/State` が Running、`ns=3;s=FastUInt1` が UInt32 として読める |

ローカルで Python 経路を動かす:

```sh
cargo build --locked -p banto-opcua-server --example interop_fixed_tags
python3 -m venv /tmp/banto-opcua-interop-venv
/tmp/banto-opcua-interop-venv/bin/python -m pip install -r interop/requirements.txt
/tmp/banto-opcua-interop-venv/bin/python interop/test_asyncua.py target/debug/examples/interop_fixed_tags
```

opc-plc 経路は Docker が必要。コンテナの endpoint は `0.0.0.0:50000` で
待ち受けさせ、ホスト側は `127.0.0.1:50000` だけへ公開する。

```sh
cargo build --locked -p banto-opcua-client --example interop_read
docker run --rm -d --name banto-opcua-interop -p 127.0.0.1:50000:50000 \
  mcr.microsoft.com/iotedge/opc-plc@sha256:3d16f104393869540403390e617a2d6afe98b3b0673fb9d8ff1533f35b6726cf \
  --ut --aa --pn=50000 --ph=0.0.0.0
target/debug/examples/interop_read opc.tcp://127.0.0.1:50000/
docker rm -f banto-opcua-interop
```

UaExpert による手動確認と OPC Foundation CTT は後続。`README.md` の
「テストの層」を参照。
