"""Transfer cloud-built installers to an existing draft; never publish the draft."""
import hashlib
import json
import os
from pathlib import Path
import plistlib
import re
import subprocess
import sys
import tempfile


def command(*args):
    return subprocess.check_output(args, text=True).strip()


def sha256(path):
    digest = hashlib.sha256()
    with path.open('rb') as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b''):
            digest.update(block)
    return digest.hexdigest()


def main():
    tag = os.environ['RELEASE_TAG']
    run = os.environ['SOURCE_RUN']
    repo = os.environ['GITHUB_REPOSITORY']
    artifact = os.environ['PACKAGE_ARTIFACT']
    assert re.fullmatch(r'v\d+\.\d+\.\d+', tag)
    assert run.isdigit()
    release = json.loads(command('gh', 'release', 'view', tag, '--repo', repo, '--json', 'isDraft,assets'))
    assert release['isDraft'], 'Only an existing draft can be modified'
    existing = {asset['name'] for asset in release['assets']}
    manifest = Path(f'cloud-build-{run}.json')
    provenance = {'tag': tag, 'sourceRun': run, 'artifact': artifact}
    if sys.argv[1] == 'check':
        provenance['status'] = 'upload-access-checked'
        manifest.write_text(json.dumps(provenance, indent=2))
        if manifest.name not in existing:
            subprocess.run(['gh', 'release', 'upload', tag, str(manifest), '--repo', repo], check=True)
        return

    packages = [p for p in Path('packages').rglob('*') if p.is_file()
                and p.name.endswith(('.dmg', '.app.tar.gz', '.exe', '.deb', '.rpm', '.AppImage'))]
    assert packages and all(tag[1:] in p.name for p in packages)
    if artifact == 'packages-universal-apple-darwin':
        expected = os.environ['EXPECTED_CERTIFICATE_SHA256'].lower()
        assert re.fullmatch(r'[0-9a-f]{64}', expected), 'Expected signing certificate fingerprint is required'
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            archives = [p for p in packages if p.name.endswith('.app.tar.gz')]
            mounted = False
            try:
                if archives:
                    subprocess.run(['tar', '-xzf', str(archives[0].resolve()), '-C', str(root)], check=True)
                    app = next(root.rglob('Mini-Term.app'))
                else:
                    dmg = next(p for p in packages if p.suffix == '.dmg')
                    mount = root / 'mount'
                    subprocess.run(['hdiutil', 'attach', '-readonly', '-nobrowse', '-mountpoint', str(mount), str(dmg.resolve())], check=True)
                    mounted = True
                    app = mount / 'Mini-Term.app'
                with (app / 'Contents/Info.plist').open('rb') as stream:
                    info = plistlib.load(stream)
                assert info['CFBundleShortVersionString'] == tag[1:]
                binary = app / 'Contents/MacOS' / info['CFBundleExecutable']
                architectures = command('lipo', '-archs', str(binary)).split()
                assert {'arm64', 'x86_64'} <= set(architectures)
                subprocess.run(['codesign', '--verify', '--deep', '--strict', str(app)], check=True)
                prefix = root / 'certificate-'
                subprocess.run(['codesign', '-d', f'--extract-certificates={prefix}', str(app)], check=True)
                certificate = sha256(Path(str(prefix) + '0'))
                assert certificate == expected, 'Signing certificate differs from the installed release'
                binary_data = binary.read_bytes()
                assert b'[resources] getrlimit:' in binary_data and b'[resources] setrlimit:' in binary_data
                provenance['macOS'] = {'architectures': architectures, 'certificateSHA256': certificate,
                                      'version': info['CFBundleShortVersionString'], 'resourceLimitCodePresent': True}
            finally:
                if mounted:
                    subprocess.run(['hdiutil', 'detach', str(root / 'mount')], check=True)
    provenance['status'] = 'verified'
    provenance['files'] = [{'name': p.name, 'sha256': sha256(p), 'size': p.stat().st_size} for p in packages]
    manifest.write_text(json.dumps(provenance, indent=2))
    new = [str(p) for p in packages if p.name not in existing]
    if new:
        subprocess.run(['gh', 'release', 'upload', tag, *new, '--repo', repo], check=True)
    subprocess.run(['gh', 'release', 'upload', tag, str(manifest), '--repo', repo, '--clobber'], check=True)
    print(json.dumps(provenance, indent=2))


if __name__ == '__main__':
    main()
