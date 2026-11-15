"""Build the pinned Windows runtime resource; never execute downloaded packages."""
import hashlib
import io
import json
from pathlib import Path, PurePosixPath
import urllib.request
import zipfile

ROOT = Path(__file__).resolve().parents[1]


def prepare():
    lock = json.loads((ROOT / "telegram/runtime.lock.json").read_text())
    cache = ROOT / "dist/telegram-cache"
    cache.mkdir(parents=True, exist_ok=True)
    files = {}
    for asset in lock["assets"]:
        cached = cache / asset["sha256"]
        if not cached.exists():
            with urllib.request.urlopen(asset["url"], timeout=90) as response:
                data = response.read(64 * 1024 * 1024 + 1)
            if len(data) > 64 * 1024 * 1024:
                raise ValueError("Oversized runtime artifact")
            cached.write_bytes(data)
        data = cached.read_bytes()
        if hashlib.sha256(data).hexdigest() != asset["sha256"]:
            raise ValueError("Runtime SHA-256 mismatch: " + asset["url"])
        with zipfile.ZipFile(io.BytesIO(data)) as archive:
            for entry in archive.infolist():
                if entry.is_dir():
                    continue
                path = PurePosixPath(entry.filename)
                if path.is_absolute() or '..' in path.parts or '\\' in entry.filename or ':' in entry.filename:
                    raise ValueError("Unsafe runtime path")
                name = entry.filename
                if asset["kind"] == "source":
                    name = '/'.join(path.parts[1:])
                    if name == 'LICENSE':
                        name = 'FLOWSEAL-LICENSE.txt'
                    elif not name.startswith('proxy/') or not name.endswith('.py'):
                        continue
                elif asset["kind"] == "python":
                    if name.lower().endswith(('.exe', '._pth', '.cat')):
                        continue
                    if name == 'LICENSE.txt':
                        name = 'PYTHON-LICENSE.txt'
                if name in files or entry.file_size > 64 * 1024 * 1024:
                    raise ValueError("Duplicate or oversized runtime entry: " + name)
                files[name] = archive.read(entry)
    files['svyaz_proxy.py'] = (ROOT / 'telegram/svyaz_proxy.py').read_bytes()
    files['runtime.lock.json'] = (ROOT / 'telegram/runtime.lock.json').read_bytes()
    manifest = {name: hashlib.sha256(data).hexdigest() for name, data in sorted(files.items())}
    files['manifest.json'] = json.dumps(manifest, sort_keys=True).encode()
    output = ROOT / 'dist/telegram-runtime.zip'
    with zipfile.ZipFile(output, 'w', compression=zipfile.ZIP_DEFLATED, compresslevel=9) as archive:
        for name, data in sorted(files.items()):
            entry = zipfile.ZipInfo(name, (2026, 1, 1, 0, 0, 0))
            entry.compress_type = zipfile.ZIP_DEFLATED
            archive.writestr(entry, data)
    print('Embedded Telegram runtime:', output.stat().st_size, 'bytes;', len(files), 'files')


if __name__ == '__main__':
    prepare()
