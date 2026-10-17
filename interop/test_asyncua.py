"""Python asyncua から固定タグの Browse / Read / Subscription と、書き込み可の
タグへの Write を確認する。"""

import asyncio
import select
import subprocess
import sys

from asyncua import Client, ua

CLIENT_URI = "urn:banto:opcua:interop-asyncua"


class Changes:
    def __init__(self):
        self.values = asyncio.Queue()

    def datachange_notification(self, node, value, data):
        self.values.put_nowait((node.nodeid.to_string(), value, data))


async def expect_line(process, prefix):
    """サーバーの標準出力から `prefix` で始まる行を待つ。ほかの行は読み飛ばす。"""
    while True:
        try:
            line = await asyncio.wait_for(asyncio.to_thread(process.stdout.readline), timeout=10)
        except TimeoutError:
            # 読み取りのスレッドは中止できないので、サーバーを止めて EOF で抜けさせる。
            # 残すと asyncio.run がスレッドの終了を待ち、失敗の報告が遅れる。
            process.kill()
            raise
        if not line:
            raise RuntimeError(f"{prefix} の前にサーバーの出力が終わりました")
        if line.startswith(prefix):
            return line.strip()


async def expect_rejection(write, error):
    try:
        await write
    except error:
        return
    raise AssertionError(f"{error.__name__} で拒否されるはず")


async def verify_writes(client, namespace, process):
    cell = await client.nodes.objects.get_child(
        [f"{namespace}:Banto", f"{namespace}:PLC2", f"{namespace}:Cell1"]
    )
    setpoint = await cell.get_child(f"{namespace}:Setpoint")
    enable = await cell.get_child(f"{namespace}:Enable")
    locked = await cell.get_child(f"{namespace}:Locked")
    assert setpoint.nodeid.Identifier == "PLC2.Cell1.Setpoint"
    read_write = ua.AccessLevel.CurrentRead.mask | ua.AccessLevel.CurrentWrite.mask
    for attribute in (ua.AttributeIds.AccessLevel, ua.AttributeIds.UserAccessLevel):
        level = await setpoint.read_attribute(attribute)
        assert level.Value.Value == read_write, level

    # write_value は SourceTimestamp を付けて送る。サーバーは時刻を捨てて受け付ける。
    await setpoint.write_value(12.5, ua.VariantType.Double)
    assert await expect_line(process, "WRITE ") == (
        f"WRITE PLC2.Cell1.Setpoint 12.5 anonymous {CLIENT_URI}"
    )
    # Write だけではノードの値は変わらず、機器から戻った値で変わる。
    assert await setpoint.read_value() == 0.0
    process.stdin.write("reflect\n")
    process.stdin.flush()
    await expect_line(process, "REFLECTED")
    assert await setpoint.read_value() == 12.5

    await enable.write_value(True)
    assert await expect_line(process, "WRITE ") == (
        f"WRITE PLC2.Cell1.Enable true anonymous {CLIENT_URI}"
    )
    process.stdin.write("reflect\n")
    process.stdin.flush()
    await expect_line(process, "REFLECTED")
    assert await enable.read_value() is True

    speed = await client.nodes.objects.get_child(
        [f"{namespace}:{name}" for name in ("Banto", "PLC1", "Line1", "Speed")]
    )
    await expect_rejection(speed.write_value(1.0), ua.uaerrors.BadNotWritable)
    await expect_rejection(setpoint.write_value(True), ua.uaerrors.BadTypeMismatch)
    await expect_rejection(locked.write_value(1.0), ua.uaerrors.BadOutOfService)


async def verify(endpoint, process):
    client = Client(url=endpoint, timeout=10)
    client.application_uri = CLIENT_URI
    async with client:
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
        # get_child は BrowsePath 変換なので、Browse サービスも直接確認する。
        parent = client.nodes.objects
        for name, identifier in (
            ("Banto", "Banto"),
            ("PLC1", "PLC1"),
            ("Line1", "PLC1.Line1"),
        ):
            references = await parent.get_children_descriptions()
            matches = [
                ref
                for ref in references
                if (ref.BrowseName.NamespaceIndex, ref.BrowseName.Name)
                == (namespace, name)
            ]
            assert len(matches) == 1, references
            assert matches[0].NodeId.NamespaceIndex == namespace
            assert matches[0].NodeId.Identifier == identifier
            parent = client.get_node(matches[0].NodeId)
        assert parent.nodeid == line.nodeid
        references = await line.get_children_descriptions()
        assert len(references) == 2, references
        children = {
            (ref.BrowseName.NamespaceIndex, ref.BrowseName.Name): ref.NodeId.Identifier
            for ref in references
        }
        assert children == {
            (namespace, "Speed"): "PLC1.Line1.Speed",
            (namespace, "Running"): "PLC1.Line1.Running",
        }, references
        assert await speed.read_value() == 0.0
        assert await running.read_value() is False

        changes = Changes()
        subscription = await client.create_subscription(100, changes)
        try:
            await subscription.subscribe_data_change([speed, running])
            process.stdin.write("update\n")
            process.stdin.flush()

            expected = {
                speed.nodeid.to_string(): (float, 42.5),
                running.nodeid.to_string(): (bool, True),
            }
            received = {}
            while len(received) < 2:
                node_id, value, data = await asyncio.wait_for(changes.values.get(), timeout=10)
                if (
                    node_id in expected
                    and type(value) is expected[node_id][0]
                    and value == expected[node_id][1]
                ):
                    assert data.monitored_item.Value.StatusCode.is_good()
                    received[node_id] = value
            assert received[speed.nodeid.to_string()] == 42.5
            assert received[running.nodeid.to_string()] is True
            assert await speed.read_value() == 42.5
            assert await running.read_value() is True
        finally:
            await subscription.delete()

        await verify_writes(client, namespace, process)


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
    print("asyncua interop passed")


if __name__ == "__main__":
    main()
