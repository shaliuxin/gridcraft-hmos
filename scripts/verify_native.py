#!/usr/bin/env python3
"""Check the packaged native ABIs and ensure the HAP contains no web runtime snapshot.

Mirrors photocraft-hmos' verify_native.py but uses GridCraft's library/bundle names.
"""
import argparse
import json
import struct
import zipfile
from pathlib import Path

NATIVE_PAGES = ['pages/Index']

# e_machine values from the ELF header. Keep in step with the triples in scripts/build_native.sh
# and the mapping in entry/src/main/cpp/CMakeLists.txt.
ELF_MACHINE = {'arm64-v8a': 183, 'x86_64': 62, 'armeabi-v7a': 40}
DEFAULT_ABIS = ['arm64-v8a', 'x86_64']
LIB_NAME = 'libgridcraft.so'


def verify(path: Path, expected_bundle: str | None = None,
           abis: list[str] | None = None) -> None:
    expected = list(abis or DEFAULT_ABIS)
    unknown = [abi for abi in expected if abi not in ELF_MACHINE]
    if unknown:
        raise ValueError(f'unknown ABIs: {unknown}')
    with zipfile.ZipFile(path) as archive:
        names = archive.namelist()
        for abi in expected:
            name = f'libs/{abi}/{LIB_NAME}'
            if name not in names:
                raise ValueError(f'missing {name}')
            data = archive.read(name)
            # 0x02 selects ELF64; every ABI here is 64-bit.
            if data[:5] != b'\x7fELF\x02' or struct.unpack_from('<H', data, 18)[0] != ELF_MACHINE[abi]:
                raise ValueError(f'incorrect ELF architecture: {name}')
        packaged = {name.split('/')[1] for name in names if name.startswith('libs/') and name.count('/') >= 2}
        extra = sorted(packaged - set(expected))
        if extra:
            raise ValueError(f'unexpected ABIs in HAP: {extra}')
        forbidden = [name for name in names if name.lower().endswith(('.wasm', '.html', '.js'))]
        if forbidden:
            raise ValueError(f'web assets remain: {forbidden}')
        pages = json.loads(archive.read('resources/base/profile/main_pages.json'))
        if pages.get('src') != NATIVE_PAGES:
            raise ValueError(f'non-native pages remain: {pages}')
        if expected_bundle is not None:
            config = json.loads(archive.read('module.json'))
            if config['app']['bundleName'] != expected_bundle:
                raise ValueError(f"unexpected bundle: {config['app']['bundleName']}")
    print(f'Native HAP verified: {path} (ABIs: {", ".join(expected)})')


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('hap', type=Path)
    parser.add_argument('--expected-bundle')
    parser.add_argument('--abis', nargs='+', metavar='ABI',
                        help=f'ABIs the HAP must contain (default: {" ".join(DEFAULT_ABIS)})')
    args = parser.parse_args()
    verify(args.hap, args.expected_bundle, args.abis)
