"""Headless lifecycle adapter for the pinned Flowseal engine (no tray imports)."""
import asyncio
import hashlib
import struct
import time
import json
import logging
import os
import threading

# Keep the DLL directory handle alive for native extensions such as _ssl and cryptography.
_dll_directory = os.add_dll_directory(os.path.dirname(__file__)) if os.name == 'nt' else None

from proxy import config
from proxy import tg_ws_proxy as engine

_thread = None
_loop = None
_task = None
_error = None
_stopping = False
_probe_future = None
_refresh_stop = threading.Event()
_refresh_lock = threading.Lock()

# Upstream logs contain proxy secrets and peer addresses; expose only error classes.
logging.getLogger('tg-mtproto-proxy').disabled = True


def _start_domain_refresh():
    global _refresh_stop
    with _refresh_lock:
        _refresh_stop.set()
        stop = _refresh_stop = threading.Event()
        config.balancer.update_domains_list(config.CFPROXY_DEFAULT_DOMAINS)
    if not config.proxy_config.fallback_cfproxy:
        return

    def refresh():
        while not stop.is_set():
            domains = config._normalize_domain_pool(config._fetch_cfproxy_domain_list())
            # An old bounded HTTP fetch may finish after stop/restart; never publish it.
            with _refresh_lock:
                if stop.is_set() or config.proxy_config.cfproxy_user_domains:
                    return
                if len(domains) >= config._CFPROXY_MIN_VALID_DOMAINS:
                    config.balancer.update_domains_list(domains)
            if stop.wait(3600):
                return

    threading.Thread(target=refresh, name='Svyaz Telegram domains', daemon=True).start()


engine.start_cfproxy_domain_refresh = _start_domain_refresh


def configure(raw):
    data = json.loads(raw)
    port, secret = data['port'], data['secret']
    if type(port) is not int or not 1 <= port <= 65535:
        raise ValueError('port')
    if not isinstance(secret, str) or len(secret) != 32 or any(c not in '0123456789abcdefABCDEF' for c in secret):
        raise ValueError('secret')
    defaults = config.ProxyConfig()
    pc = config.proxy_config
    pc.host, pc.port, pc.secret = '127.0.0.1', port, secret
    pc.dc_redirects = config.parse_dc_ip_list(data.get('dc_ip', [f'{dc}:{ip}' for dc, ip in defaults.dc_redirects.items()]))
    for key, attr, default in [('buf_kb', 'buffer_size', 256), ('pool_size', 'pool_size', 4)]:
        value = data.get(key, default)
        if type(value) is not int or not 0 <= value <= (4096 if key == 'buf_kb' else 64):
            raise ValueError(key)
        setattr(pc, attr, max(4, value) * 1024 if key == 'buf_kb' else value)
    for key, attr in [('cfproxy', 'fallback_cfproxy'), ('h2', 'cfproxy_h2_media'), ('no_secure', 'disable_secure'), ('force_test_dc', 'force_test_dc')]:
        value = data.get(key, getattr(defaults, attr))
        if type(value) is not bool:
            raise ValueError(key)
        setattr(pc, attr, value)
    for key, attr, enabled_key in [('cfproxy_user_domain', 'cfproxy_user_domains', 'cfproxy_user_domain_enabled'), ('cfproxy_worker_domain', 'cfproxy_worker_domains', 'cfproxy_worker_enabled')]:
        domains = config.coerce_domain_list(data.get(key, []))
        enabled = data.get(enabled_key, bool(domains))
        if type(enabled) is not bool:
            raise ValueError(enabled_key)
        setattr(pc, attr, domains if enabled else [])
    pc.fake_tls_domain, pc.proxy_protocol = '', False


def start(raw):
    global _thread, _error, _stopping, _probe_future
    if _thread is not None and _thread.is_alive():
        raise RuntimeError('already running')
    configure(raw)
    _probe_future = None
    _error, _stopping = None, False
    _thread = threading.Thread(target=_run, name='Svyaz Telegram', daemon=True)
    _thread.start()


def _run():
    global _loop, _task, _error
    async def serve():
        global _loop, _task
        _loop, _task = asyncio.get_running_loop(), asyncio.current_task()
        try:
            if not _stopping:
                await engine._run()
        finally:
            # Upstream's cleanup begins after warmup; cover cancellation before it.
            if engine._server_instance is not None:
                engine._server_instance.close()
                engine._server_instance.abort_clients()
                clients = list(engine._client_tasks)
                for task in clients:
                    task.cancel()
                await asyncio.gather(*clients, return_exceptions=True)
                await engine._server_instance.wait_closed()
            await engine.ws_pool.close()
            await engine.cf_worker_pool.close()
            if engine.cf_h2_pool is not None:
                await engine.cf_h2_pool.close()
                engine.cf_h2_pool = None
            await asyncio.sleep(0)
    try:
        asyncio.run(serve())
    except asyncio.CancelledError:
        pass
    except BaseException as error:
        _error = type(error).__name__
    finally:
        with _refresh_lock:
            _refresh_stop.set()
        config._refresh_stop.set()
        # Cancellation can arrive during upstream warmup, before its try/finally.
        engine._server_instance = None
        _loop, _task = None, None


def stop():
    global _stopping
    _stopping = True
    cancel_probe()
    with _refresh_lock:
        _refresh_stop.set()
    if _loop is not None and _task is not None:
        try:
            _loop.call_soon_threadsafe(_task.cancel)
        except RuntimeError:
            pass
    config._refresh_stop.set()


def status():
    server = engine._server_instance
    alive = _thread is not None and _thread.is_alive()
    probe = 'idle'
    elapsed_ms = None
    if _probe_future is not None and not _probe_future.cancelled():
        if not _probe_future.done():
            probe = 'checking'
        else:
            try:
                result = _probe_future.result()
                probe = 'passed' if result['success'] else 'failed'
                elapsed_ms = result['elapsed_ms']
            except Exception:
                probe = 'failed'
    return json.dumps({'alive': alive, 'ready': alive and not _stopping and server is not None and bool(server.sockets), 'error': _error, 'probe': probe, 'probeElapsedMs': elapsed_ms})


async def _probe_server(timeout=12):
    """Anonymous MTProto req_pq_multi through our listener; no account or auth key."""
    writer = None
    try:
        async with asyncio.timeout(timeout):
            pc = config.proxy_config
            secret = bytes.fromhex(pc.secret)
            initial = bytearray(os.urandom(64))
            initial[0] = 0x10
            initial[4] |= 1
            initial[56:60] = b'\xee' * 4
            dc = 2 if 2 in pc.dc_redirects else next(iter(pc.dc_redirects), 2)
            initial[60:62] = struct.pack('<h', dc)
            outgoing = engine.Cipher(engine.algorithms.AES(hashlib.sha256(initial[8:40] + secret).digest()),
                                     engine.modes.CTR(bytes(initial[40:56]))).encryptor()
            encrypted = outgoing.update(bytes(initial))
            reverse = bytes(initial[8:56])[::-1]
            incoming = engine.Cipher(engine.algorithms.AES(hashlib.sha256(reverse[:32] + secret).digest()),
                                     engine.modes.CTR(reverse[32:])).encryptor()
            nonce = os.urandom(16)
            body = struct.pack('<I', 0xbe7e8ef1) + nonce
            message = struct.pack('<QQI', 0, int(time.time() * (1 << 32)) & ~3, len(body)) + body
            reader, writer = await asyncio.open_connection('127.0.0.1', pc.port)
            writer.write(bytes(initial[:56]) + encrypted[56:] + outgoing.update(struct.pack('<I', len(message)) + message))
            await writer.drain()
            length = struct.unpack('<I', incoming.update(await reader.readexactly(4)))[0]
            if not 56 <= length <= 4096 or length % 4:
                return False
            reply = incoming.update(await reader.readexactly(length))
            return (reply[:8] == bytes(8) and struct.unpack_from('<I', reply, 16)[0] == length - 20
                    and reply[20:24] == struct.pack('<I', 0x05162463) and reply[24:40] == nonce)
    except (OSError, TimeoutError, asyncio.IncompleteReadError, ValueError):
        return False
    finally:
        if writer is not None:
            writer.close()
            await writer.wait_closed()


def start_probe():
    global _probe_future
    cancel_probe()
    if _loop is None or _stopping:
        raise RuntimeError('not running')
    _probe_future = asyncio.run_coroutine_threadsafe(_timed_probe(), _loop)


def cancel_probe():
    if _probe_future is not None and not _probe_future.done():
        _probe_future.cancel()


async def _timed_probe():
    started = time.monotonic()
    success = await _probe_server()
    return {'success': success, 'elapsed_ms': int((time.monotonic() - started) * 1000) if success else None}
