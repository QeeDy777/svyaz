import asyncio
import gc
import hashlib
import json
import os
from pathlib import Path
import socket
import struct
import sys
import threading
import time
import unittest
import warnings
from unittest.mock import AsyncMock, patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'telegram'))
import svyaz_proxy as host
from proxy._aes import Cipher, algorithms, modes


class EmbeddedChecks(unittest.TestCase):
    def setUp(self):
        self.refresh = patch.object(host.config, 'start_cfproxy_domain_refresh')
        # Engine imported this function directly.
        self.engine_refresh = patch.object(host.engine, 'start_cfproxy_domain_refresh')
        self.warmup = patch.object(host.engine.ws_pool, 'warmup', new_callable=AsyncMock)
        self.worker_warmup = patch.object(host.engine.cf_worker_pool, 'warmup', new_callable=AsyncMock)
        for item in (self.refresh, self.engine_refresh, self.warmup, self.worker_warmup):
            item.start()
            self.addCleanup(item.stop)
        with socket.socket() as sock:
            sock.bind(('127.0.0.1', 0))
            self.port = sock.getsockname()[1]
        self.secret = os.urandom(16).hex()
        self.raw = json.dumps({'port': self.port, 'secret': self.secret, 'host': '0.0.0.0', 'pool_size': 0, 'cfproxy': False})
        self.addCleanup(self.stop)

    def until(self, predicate):
        deadline = time.monotonic() + 5
        while not predicate():
            if time.monotonic() > deadline:
                self.fail('Timed out: ' + host.status())
            time.sleep(.01)

    def stop(self):
        host.stop()
        self.until(lambda: not json.loads(host.status())['alive'])

    def assert_port_released(self):
        with socket.socket() as sock:
            if os.name != 'nt':
                sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
            sock.bind(('127.0.0.1', self.port))
            sock.listen()

    def test_start_stop_restart_and_loopback(self):
        for _ in range(3):
            host.start(self.raw)
            self.until(lambda: json.loads(host.status())['ready'])
            self.assertEqual(host.config.proxy_config.host, '127.0.0.1')
            with socket.create_connection(('127.0.0.1', self.port), timeout=2) as sock:
                sock.sendall(b'invalid handshake')
            self.stop()
            self.assert_port_released()
        self.assertNotIn('ui', sys.modules)
        self.assertNotIn('windows', sys.modules)

    def test_busy_port_and_private_error(self):
        with socket.socket() as foreign:
            foreign.bind(('127.0.0.1', self.port))
            foreign.listen()
            host.start(self.raw)
            self.until(lambda: not json.loads(host.status())['alive'])
            self.assertEqual(json.loads(host.status())['error'], 'OSError')
            self.assertNotIn(self.secret, host.status())
            with socket.create_connection(('127.0.0.1', self.port), timeout=2):
                pass

    def test_cancel_during_warmup_releases_listener(self):
        async def slow():
            await asyncio.sleep(60)
        with patch.object(host.engine.ws_pool, 'warmup', side_effect=slow):
            host.start(self.raw)
            self.until(lambda: json.loads(host.status())['ready'])
            with socket.create_connection(('127.0.0.1', self.port), timeout=2) as client:
                client.sendall(b'\x10')
                self.until(lambda: bool(host.engine._client_tasks))
                self.stop()
                self.assertEqual(client.recv(1), b'')
            self.assert_port_released()

    def test_stale_domain_fetch_cannot_replace_new_configuration(self):
        fetching, release = threading.Event(), threading.Event()
        def fetch():
            fetching.set()
            release.wait(3)
            return ['old-a.example.com', 'old-b.example.com', 'old-c.example.com']
        with patch.object(host.config, '_fetch_cfproxy_domain_list', side_effect=fetch), patch.object(host.config.balancer, 'update_domains_list') as update:
            host.config.proxy_config.fallback_cfproxy = True
            host.config.proxy_config.cfproxy_user_domains = []
            host._start_domain_refresh()
            self.assertTrue(fetching.wait(2))
            worker = next(t for t in threading.enumerate() if t.name == 'Svyaz Telegram domains')
            host.stop()
            host.config.proxy_config.cfproxy_user_domains = ['chosen.example.com']
            update.reset_mock()
            release.set()
            worker.join(2)
            self.assertFalse(worker.is_alive())
            update.assert_not_called()

    def test_disabled_custom_relays_stay_disabled(self):
        data = json.loads(self.raw)
        data.update(cfproxy_worker_domain=['disabled.example.com'], cfproxy_worker_enabled=False,
                    cfproxy_user_domain=['disabled.example.org'], cfproxy_user_domain_enabled=False)
        host.configure(json.dumps(data))
        self.assertEqual(host.config.proxy_config.cfproxy_worker_domains, [])
        self.assertEqual(host.config.proxy_config.cfproxy_user_domains, [])
        data['cfproxy_worker_enabled'] = True
        host.configure(json.dumps(data))
        self.assertEqual(host.config.proxy_config.cfproxy_worker_domains, ['disabled.example.com'])

    def test_invalid_settings_rejected_before_thread(self):
        for value in ({'port': True, 'secret': self.secret}, {'port': self.port, 'secret': 'x' * 32}, {'port': self.port, 'secret': self.secret, 'pool_size': 9999}):
            with self.assertRaises(ValueError):
                host.start(json.dumps(value))
        self.assertFalse(json.loads(host.status())['alive'])

    def test_mtproto_handshake_secret_and_media_dc(self):
        # Independently construct Telegram's obfuscated init; adapter uses original parser.
        initial = bytearray(os.urandom(64))
        initial[56:60] = b'\xdd' * 4
        initial[60:62] = struct.pack('<h', -4)
        key = hashlib.sha256(bytes(initial[8:40]) + bytes.fromhex(self.secret)).digest()
        encryptor = Cipher(algorithms.AES(key), modes.CTR(bytes(initial[40:56]))).encryptor()
        encrypted = encryptor.update(bytes(initial))
        wire = bytes(initial[:56]) + encrypted[56:]
        result = host.engine._try_handshake(wire, bytes.fromhex(self.secret))
        self.assertEqual(result[:3], (4, True, b'\xdd' * 4))
        self.assertIsNone(host.engine._try_handshake(wire, bytes(16)))

    def test_encrypted_payload_round_trip_through_original_engine(self):
        class EchoDc:
            def __init__(self):
                self.decryptor = None
                self.queue = asyncio.Queue()

            async def send(self, data):
                if self.decryptor is None:
                    self.decryptor = Cipher(algorithms.AES(data[8:40]), modes.CTR(data[40:56])).encryptor()
                    self.decryptor.update(bytes(64))
                    reverse = data[8:56][::-1]
                    self.encryptor = Cipher(algorithms.AES(reverse[:32]), modes.CTR(reverse[32:])).encryptor()
                else:
                    await self.queue.put(self.encryptor.update(self.decryptor.update(data)))

            async def send_batch(self, frames):
                for frame in frames:
                    await self.send(frame)

            async def recv(self):
                return await self.queue.get()

            async def close(self):
                pass

        async def connect(*args, **kwargs):
            return EchoDc()

        with patch.object(host.engine.ws_pool, 'get', side_effect=connect):
            host.start(self.raw)
            self.until(lambda: json.loads(host.status())['ready'])
            initial = bytearray(os.urandom(64))
            initial[0] = 0x10
            initial[56:60] = b'\xee' * 4
            initial[60:62] = struct.pack('<h', 2)
            secret = bytes.fromhex(self.secret)
            key = hashlib.sha256(bytes(initial[8:40]) + secret).digest()
            outgoing = Cipher(algorithms.AES(key), modes.CTR(bytes(initial[40:56]))).encryptor()
            encrypted = outgoing.update(bytes(initial))
            reverse = bytes(initial[8:56])[::-1]
            incoming = Cipher(algorithms.AES(hashlib.sha256(reverse[:32] + secret).digest()), modes.CTR(reverse[32:])).encryptor()
            packet = struct.pack('<I', 16) + b'svyaz round trip'
            with socket.create_connection(('127.0.0.1', self.port), timeout=3) as sock:
                sock.sendall(bytes(initial[:56]) + encrypted[56:] + outgoing.update(packet))
                reply = b''
                while len(reply) < len(packet):
                    chunk = sock.recv(len(packet) - len(reply))
                    self.assertTrue(chunk)
                    reply += chunk
                self.assertEqual(incoming.update(reply), packet)

    def test_server_probe_through_engine_and_invalid_nonce(self):
        class ReplyDc:
            def __init__(self):
                self.dec = None
                self.queue = asyncio.Queue()
            async def send(self, data):
                if self.dec is None:
                    self.dec = Cipher(algorithms.AES(data[8:40]), modes.CTR(data[40:56])).encryptor()
                    header = self.dec.update(data)
                    assert header[56:60] == bytes([0xdd]) * 4
                    reverse = data[8:56][::-1]
                    self.enc = Cipher(algorithms.AES(reverse[:32]), modes.CTR(reverse[32:])).encryptor()
                else:
                    request = self.dec.update(data)
                    assert request[24:28] == struct.pack('<I', 0xbe7e8ef1)
                    nonce = request[28:44] if valid else bytes(16)
                    body = struct.pack('<I', 0x05162463) + nonce + os.urandom(16) + bytes(20)
                    message = struct.pack('<QQI', 0, 1, len(body)) + body + bytes(padding)
                    await self.queue.put(self.enc.update(struct.pack('<I', len(message)) + message))
            async def send_batch(self, frames):
                for frame in frames:
                    await self.send(frame)
            async def recv(self):
                return await self.queue.get()
            async def close(self):
                pass
        async def connect(*args, **kwargs):
            return ReplyDc()
        with patch.object(host.engine.ws_pool, 'get', side_effect=connect):
            host.start(self.raw)
            self.until(lambda: json.loads(host.status())['ready'])
            for valid, padding, expected in [(True, 0, 'passed'), (True, 3, 'passed'), (True, 15, 'passed'),
                                             (False, 3, 'failed'), (True, 16, 'failed')]:
                host.start_probe()
                self.until(lambda: json.loads(host.status())['probe'] != 'checking')
                self.assertEqual(json.loads(host.status())['probe'], expected)
                elapsed = json.loads(host.status())['probeElapsedMs']
                self.assertTrue(elapsed is not None and elapsed >= 0 if expected == 'passed' else elapsed is None)
            self.stop()
            self.assert_port_released()

    def test_probe_timeout_and_cancel_release_socket(self):
        async def scenario():
            closed = asyncio.Event()
            async def silent(reader, writer):
                try:
                    await reader.read()
                finally:
                    writer.close()
                    await writer.wait_closed()
                    closed.set()
            server = await asyncio.start_server(silent, '127.0.0.1', self.port)
            host.configure(self.raw)
            async with server:
                self.assertFalse(await host._probe_server(.05))
                await asyncio.wait_for(closed.wait(), 1)
                closed.clear()
                task = asyncio.create_task(host._probe_server())
                await asyncio.sleep(.02)
                task.cancel()
                with self.assertRaises(asyncio.CancelledError):
                    await task
                await asyncio.wait_for(closed.wait(), 1)
        asyncio.run(scenario())


if __name__ == '__main__':
    if '--stress' in sys.argv:
        unraisable = []
        sys.unraisablehook = lambda event: unraisable.append(str(event.exc_value))
        warnings.simplefilter('error', ResourceWarning)
        for iteration in range(30):
            result = unittest.TestResult()
            unittest.defaultTestLoader.loadTestsFromTestCase(EmbeddedChecks).run(result)
            gc.collect()
            if not result.wasSuccessful() or unraisable:
                print(result.errors, result.failures, unraisable)
                raise SystemExit(1)
        print(f'{30 * result.testsRun} checks passed; no leaked transports or unraisable cleanup errors')
    else:
        unittest.main()
