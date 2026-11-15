"""Headless lifecycle adapter for the pinned Flowseal engine (no tray imports)."""
import asyncio
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

# Upstream logs contain proxy secrets and peer addresses; expose only error classes.
logging.getLogger('tg-mtproto-proxy').disabled = True


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
    for key, attr in [('cfproxy_user_domain', 'cfproxy_user_domains'), ('cfproxy_worker_domain', 'cfproxy_worker_domains')]:
        domains = config.coerce_domain_list(data.get(key, []))
        setattr(pc, attr, domains if data.get(key + '_enabled', bool(domains)) else [])
    pc.fake_tls_domain, pc.proxy_protocol = '', False


def start(raw):
    global _thread, _error, _stopping
    if _thread is not None and _thread.is_alive():
        raise RuntimeError('already running')
    configure(raw)
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
                await engine._server_instance.wait_closed()
            await engine.ws_pool.close()
            await engine.cf_worker_pool.close()
            if engine.cf_h2_pool is not None:
                await engine.cf_h2_pool.close()
                engine.cf_h2_pool = None
    try:
        asyncio.run(serve())
    except asyncio.CancelledError:
        pass
    except BaseException as error:
        _error = type(error).__name__
    finally:
        config._refresh_stop.set()
        # Cancellation can arrive during upstream warmup, before its try/finally.
        engine._server_instance = None
        _loop, _task = None, None


def stop():
    global _stopping
    _stopping = True
    if _loop is not None and _task is not None:
        try:
            _loop.call_soon_threadsafe(_task.cancel)
        except RuntimeError:
            pass
    config._refresh_stop.set()


def status():
    server = engine._server_instance
    alive = _thread is not None and _thread.is_alive()
    return json.dumps({'alive': alive, 'ready': alive and not _stopping and server is not None and bool(server.sockets), 'error': _error})
