import importlib.util
from pathlib import Path
import hashlib
import subprocess
import tarfile
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parent.parent
spec = importlib.util.spec_from_file_location('packaging_tools', ROOT / 'scripts/package.py')
packaging_tools = importlib.util.module_from_spec(spec)
spec.loader.exec_module(packaging_tools)

class PackagingTests(unittest.TestCase):
    def test_formula_checksums_are_derived_from_each_archive(self):
        with tempfile.TemporaryDirectory() as temp:
            folder = Path(temp)
            for target in packaging_tools.TARGETS:
                (folder / f'agentbanter-{packaging_tools.version()}-{target}.tar.gz').write_bytes(target.encode())
            formula = folder / 'agentbanter.rb'
            packaging_tools.formula(folder, formula, 'BuenGenio/AgentBanter')
            text = formula.read_text()
            for target in packaging_tools.TARGETS:
                self.assertIn(hashlib.sha256(target.encode()).hexdigest(), text)
            self.assertIn('service do', text)
            self.assertEqual(len((folder / 'SHA256SUMS').read_text().splitlines()), 4)
            if __import__('shutil').which('ruby'):
                subprocess.run(['ruby', '-c', str(formula)], check=True, capture_output=True)
    def test_archive_includes_installer_and_third_party_licenses(self):
        with tempfile.TemporaryDirectory() as temp:
            folder = Path(temp)
            binary = folder / 'binary'
            binary.write_bytes(b'test fixture, not a release executable')
            with patch.object(packaging_tools, 'rust_licenses'):
                archive = packaging_tools.package(binary, 'aarch64-apple-darwin', folder / 'dist')
            with tarfile.open(archive) as tar:
                names = [Path(name).name for name in tar.getnames()]
                for expected in ['agentbanter', 'install.sh', 'LICENSE', 'acorn.LICENSE', 'highlight.LICENSE']:
                    self.assertIn(expected, names)
            digest = hashlib.sha256(archive.read_bytes()).hexdigest()
            self.assertIn(digest, archive.with_suffix('.gz.sha256').read_text())
    def test_formula_requires_all_archives(self):
        with tempfile.TemporaryDirectory() as temp:
            with self.assertRaises(FileNotFoundError):
                packaging_tools.formula(Path(temp), Path(temp) / 'agentbanter.rb', 'BuenGenio/AgentBanter')

if __name__ == '__main__':
    unittest.main()
