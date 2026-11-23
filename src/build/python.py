import importlib.metadata
import os
from pathlib import Path, PurePosixPath
import shlex
import subprocess
import sys
from zipfile import ZipFile

from installer import install
from installer.destinations import SchemeDictionaryDestination
from installer.sources import WheelFile
from packaging.requirements import Requirement
from packaging.tags import sys_tags
from packaging.utils import canonicalize_name, parse_wheel_filename

prefix, roots, imports, native_imports, pytest = sys.argv[1:]
prefix = Path(prefix)
env = dict(os.environ, PYTHONNOUSERSITE="1", PYTHONDONTWRITEBYTECODE="1")
wheels = Path("/build/wheels")
subprocess.run(
    [sys.executable, "-sB", "-m", "build", "--wheel", "--no-isolation", "--outdir", wheels],
    env=env,
    check=True,
)
(wheel,) = wheels.glob("*.whl")
_, _, _, tags = parse_wheel_filename(wheel.name)
if tags.isdisjoint(sys_tags()):
    sys.exit(f"wheel tags are incompatible with {sys.executable}: {wheel.name}")

with ZipFile(wheel) as archive:
    seen = set()
    for item in archive.infolist():
        path = PurePosixPath(item.filename)
        if (
            not path.parts
            or path.is_absolute()
            or ".." in path.parts
            or path in seen
            or "\\" in item.filename
            or str(path) != item.filename.rstrip("/")
        ):
            sys.exit(f"unsafe or duplicate wheel member: {item.filename}")
        seen.add(path)

class Destination(SchemeDictionaryDestination):
    def write_script(self, name, module, attr, section):
        if not name or name in {".", ".."} or "/" in name or "\\" in name:
            sys.exit(f"invalid wheel script name: {name!r}")
        return super().write_script(name, module, attr, section)


site = Path("lib") / f"python{sys.version_info.major}.{sys.version_info.minor}" / "site-packages"
schemes = {"purelib": site, "platlib": site, "scripts": "bin", "headers": "include", "data": "."}
with WheelFile.open(wheel) as source:
    source.validate_record()
    install(
        source,
        Destination({k: str(prefix / v) for k, v in schemes.items()}, sys.executable, "posix"),
        {},
    )

runtime = [str(root / site) for root in [prefix, *map(Path, filter(None, roots.split(":")))] if (root / site).is_dir()]
installed = list(importlib.metadata.distributions(path=[str(prefix / site)]))
if len(installed) != 1:
    sys.exit(f"expected one installed wheel distribution, got {len(installed)}")
available = {}
for dist in importlib.metadata.distributions(path=runtime):
    available.setdefault(canonicalize_name(dist.metadata["Name"]), dist.version)
for value in installed[0].requires or ():
    requirement = Requirement(value)
    if requirement.marker and not requirement.marker.evaluate({"extra": ""}):
        continue
    version = available.get(canonicalize_name(requirement.name))
    if requirement.url or version is None or not requirement.specifier.contains(version, prereleases=True):
        sys.exit(f"unsatisfied wheel runtime requirement {value!r}: add a matching distribution to dependencies.run")

env["PYTHONPATH"] = os.pathsep.join(runtime)
checks = """
import ctypes
import importlib
from pathlib import Path
from importlib.machinery import EXTENSION_SUFFIXES
import sys
for path in sorted(Path(sys.argv[3]).rglob('*.so')):
    if '.cpython-' in path.name and not path.name.endswith(EXTENSION_SUFFIXES[0]):
        sys.exit(f'{path}: incompatible CPython extension ABI')
    ctypes.CDLL(str(path))
for name in filter(None, sys.argv[1].split(',')):
    importlib.import_module(name)
for name in filter(None, sys.argv[2].split(',')):
    origin = importlib.import_module(name).__spec__.origin or ''
    if not origin.endswith(tuple(EXTENSION_SUFFIXES)):
        sys.exit(f'{name}: expected a native extension, got {origin}')
"""
subprocess.run(
    [sys.executable, "-sBP", "-c", checks, imports, native_imports, prefix / site],
    env=env,
    cwd="/build",
    check=True,
)

if pytest == "true":
    env["PYTHONPATH"] = os.pathsep.join([env["PYTHONPATH"], os.environ.get("PYTHONPATH", "")]).rstrip(":")
    subprocess.run([sys.executable, "-sBP", "-m", "pytest", "--import-mode=importlib", "-p", "no:cacheprovider", "/build/src"], env=env, cwd="/build", check=True)
    env["PYTHONPATH"] = os.pathsep.join(runtime)

bin_dir = prefix / "bin"
if bin_dir.is_dir():
    libexec = prefix / "libexec" / "vug-python"
    libexec.mkdir(parents=True)
    for script in sorted(bin_dir.iterdir()):
        if not script.is_file() or script.is_symlink():
            sys.exit(f"expected a Python script in bin: {script.name}")
        with script.open("rb") as stream:
            shebang = stream.readline()
        if not shebang.startswith(b"#!") or b"python" not in shebang:
            sys.exit(f"expected a Python shebang: {script.name}")
        target = libexec / script.name
        script.rename(target)
        script.write_text(
            "#!/bin/sh\n"
            f"export PYTHONPATH={shlex.quote(env['PYTHONPATH'])}\n"
            "export PYTHONNOUSERSITE=1 PYTHONDONTWRITEBYTECODE=1\n"
            f"exec {shlex.quote(sys.executable)} -sBP {shlex.quote(str(target))} \"$@\"\n"
        )
        script.chmod(0o755)
