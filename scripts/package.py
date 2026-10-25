#!/usr/bin/env python3
"""Build portable archives and a Homebrew formula from actual artifact checksums."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import tarfile
import tempfile
import shutil
import subprocess


ROOT = Path(__file__).resolve().parent.parent
TARGETS = {
    'aarch64-apple-darwin': ('macos', 'arm'),
    'x86_64-apple-darwin': ('macos', 'intel'),
    'aarch64-unknown-linux-musl': ('linux', 'arm'),
    'x86_64-unknown-linux-musl': ('linux', 'intel'),
}

def version():
    package = (ROOT / 'Cargo.toml').read_text().split('[package]', 1)[1].split('\n[', 1)[0]
    return re.search(r'^version\s*=\s*"([^"]+)"', package, re.MULTILINE)[1]

def rust_licenses(destination):
    metadata = json.loads(subprocess.check_output(
        ['cargo', 'metadata', '--format-version', '1', '--locked'], cwd=ROOT))
    notices = []
    for crate in metadata['packages']:
        if crate['name'] == 'agentbanter':
            continue
        name = crate['name'] + '-' + crate['version']
        notices.append({'package': name, 'license': crate.get('license'), 'repository': crate.get('repository')})
        source = Path(crate['manifest_path']).parent
        files = [p for p in source.iterdir() if p.is_file() and p.name.upper().startswith(('LICENSE', 'LICENCE', 'COPYING', 'UNLICENSE'))]
        if crate.get('license_file'):
            files.append(source / crate['license_file'])
        if files:
            target = destination / name
            target.mkdir()
            for path in set(files):
                if path.is_file():
                    shutil.copy2(path, target / path.name)
    (destination / 'RUST-DEPENDENCIES.json').write_text(json.dumps(notices, indent=2) + '\n')


def package(binary, target, output):
    name = f'agentbanter-{version()}-{target}'
    output.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory() as temp:
        folder = Path(temp) / name
        folder.mkdir()
        shutil.copy2(binary, folder / 'agentbanter')
        (folder / 'agentbanter').chmod(0o755)
        for source in ['README.md', 'LICENSE', 'packaging/agentbanter.svg',
                       'packaging/agentbanter.desktop', 'packaging/install.sh']:
            shutil.copy2(ROOT / source, folder / Path(source).name)
        (folder / 'install.sh').chmod(0o755)
        licenses = folder / 'licenses'
        licenses.mkdir()
        rust_licenses(licenses)
        (folder / 'docs').mkdir()
        shutil.copy2(ROOT / 'docs/packaging.md', folder / 'docs/packaging.md')
        for source in (ROOT / 'static/vendor').glob('*LICENSE*'):
            shutil.copy2(source, licenses / source.name)
        archive = output / f'{name}.tar.gz'
        with tarfile.open(archive, 'w:gz') as tar:
            tar.add(folder, arcname=name)
    checksum = hashlib.sha256(archive.read_bytes()).hexdigest()
    archive.with_suffix(archive.suffix + '.sha256').write_text(f'{checksum}  {archive.name}\n')
    print(archive)
    return archive


def formula(artifacts, output, repository):
    if not re.fullmatch(r'[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+', repository):
        raise ValueError('repository must be OWNER/REPO')
    release = version()
    if not re.fullmatch(r'\d+\.\d+\.\d+(?:-[A-Za-z0-9.-]+)?', release):
        raise ValueError('invalid release version')
    result = [
        'class Agentbanter < Formula',
        '  desc "Inspect agent conversations, tool steps, and activity"',
        f'  homepage "https://github.com/{repository}"',
        f'  version "{release}"',
        '  license "MIT"', '',
    ]
    checksums = []
    for target, (system, arch) in TARGETS.items():
        archive = artifacts / f'agentbanter-{release}-{target}.tar.gz'
        digest = hashlib.sha256(archive.read_bytes()).hexdigest()
        checksums.append(f'{digest}  {archive.name}\n')
        result += [f'  on_{system} do', f'    on_{arch} do',
                   f'      url "https://github.com/{repository}/releases/download/v{release}/{archive.name}"',
                   f'      sha256 "{digest}"', '    end', '  end', '']
    result += [
        '  def install',
        '    bin.install "agentbanter"',
        '    if OS.linux?',
        '      inreplace "agentbanter.desktop", "Exec=agentbanter open", "Exec=#{opt_bin}/agentbanter open"',
        '      (share/"applications").install "agentbanter.desktop"',
        '      (share/"icons/hicolor/scalable/apps").install "agentbanter.svg"',
        '    end',
        '    (share/"agentbanter/licenses").install Dir["licenses/*"]',
        '  end', '',
        '  service do',
        '    run [opt_bin/"agentbanter", "serve"]',
        '    keep_alive true',
        '    log_path var/"log/agentbanter.log"',
        '    error_log_path var/"log/agentbanter.log"',
        '  end', '',
        '  def caveats',
        '    <<~EOS',
        '      Open the dashboard: agentbanter open',
        '      Create settings:    agentbanter config init',
        '      Optional service:   brew services start agentbanter',
        '      Restart the service after changing configuration.',
        '    EOS',
        '  end', '',
        '  test do',
        '    assert_match version.to_s, shell_output("#{bin}/agentbanter --version")',
        '    assert_match "serve", shell_output("#{bin}/agentbanter --help")',
        '  end',
        'end', '',
    ]
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text('\n'.join(result))
    (artifacts / 'SHA256SUMS').write_text(''.join(checksums))
    print(output)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='command', required=True)
    commands.add_parser('version')
    build = commands.add_parser('archive')
    build.add_argument('--binary', required=True, type=Path)
    build.add_argument('--target', required=True, choices=TARGETS)
    build.add_argument('--output', default=Path('dist'), type=Path)
    brew = commands.add_parser('formula')
    brew.add_argument('--artifacts', default=Path('dist'), type=Path)
    brew.add_argument('--output', default=Path('dist/agentbanter.rb'), type=Path)
    brew.add_argument('--repository', default='BuenGenio/AgentBanter')
    args = parser.parse_args()
    if args.command == 'version':
        print(version())
    elif args.command == 'archive':
        package(args.binary, args.target, args.output)
    else:
        formula(args.artifacts, args.output, args.repository)

if __name__ == '__main__':
    main()
