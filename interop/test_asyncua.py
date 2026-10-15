"""Python asyncua から固定タグの Browse / Read / Subscription を確認する。"""

import asyncio
import select
import subprocess
import sys

from asyncua import Client


class Changes:
    def __init__(self):
        self.values = asyncio.Queue()

    def datachange_notification(self, node, value, data):
        self.values.put_nowait((node.nodeid.to_string(), value, data))


async def verify(endpoint, process):
    async with Client(url=endpoint, timeout=10) as client:
        endpoints = await client.get_endpoints()
        assert any(item.EndpointUrl == endpoint for item in endpoints), endpoints

        namespace = await client.get_namespace_index("urn:banto:opcua:tags")
        line = await client.nodes.objects.get_child(
            [f"{namespace}:Banto", f"{namespace}:PLC1", f"{namespace}:Line1"]
        )
        speed = await line.get_child(f"{namespace}:Speed")
        running = await line.get_child(f"{namespace}:Running")
        assert speed.nodeid.Identifier == "PLC1.Line1.Speed"
        assert running.nodeid.Identifier == "PLC1.Line1.Running"
        assert await speed.read_value() == 0.0
        assert await running.read_value() is False

        changes = Changes()
        subscription = await client.create_subscription(100, changes)
        try:
            await subscription.subscribe_data_change([speed, running])
            process.stdin.write("update\n")
            process.stdin.flush()

            received = {}
            while len(received) < 2:
                node_id, value, data = await asyncio.wait_for(changes.values.get(), timeout=10)
                if value in (42.5, True):
                    assert data.monitored_item.Value.StatusCode.is_good()
                    received[node_id] = value
            assert received[speed.nodeid.to_string()] == 42.5
            assert received[running.nodeid.to_string()] is True
            assert await speed.read_value() == 42.5
            assert await running.read_value() is True
        finally:
            await subscription.delete()


def main():
    if len(sys.argv) != 2:
        raise SystemExit("usage: test_asyncua.py <interop_fixed_tags executable>")
    process = subprocess.Popen(
        [sys.argv[1]], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True
    )
    try:
        ready, _, _ = select.select([process.stdout], [], [], 30)
        if not ready:
            raise TimeoutError("サーバー起動の待ち時間が切れました")
        line = process.stdout.readline().strip()
        if not line.startswith("ENDPOINT="):
            raise RuntimeError(f"サーバーの接続先がありません: {line!r}")
        asyncio.run(verify(line.removeprefix("ENDPOINT="), process))
        print("asyncua interop passed")
    finally:
        if process.poll() is None:
            try:
                process.stdin.write("quit\n")
                process.stdin.flush()
                process.wait(timeout=15)
            except (BrokenPipeError, subprocess.TimeoutExpired):
                process.kill()
                process.wait()
        if process.returncode != 0:
            raise RuntimeError(f"サーバーが異常終了しました: {process.returncode}")


if __name__ == "__main__":
    main()
