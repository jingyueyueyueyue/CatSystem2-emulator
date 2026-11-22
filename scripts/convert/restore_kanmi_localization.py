"""Recover CatSystem2 localization data from the supplied Kanmi patch."""

from argparse import ArgumentParser
from hashlib import md5, sha256
from pathlib import Path
from struct import iter_unpack, pack, unpack_from
from tempfile import NamedTemporaryFile
from zlib import crc32, decompressobj
import re


LAUNCHER_SHA256 = 'c264b2465889c78723079a694d2654ef1091253db657a735e92f58b75031aa26'
BASE_SCENE_SHA256 = '7887397c1c7c252e71f496977b80bc68f85be637eb678bdb6f9aa4a94e1b111f'
CATALOG_KEY = md5(b'g46dgsfet567etwh501bhsd-=352').digest()
ARCHIVE = 'kanmi_cn.int'
MANIFEST = b'{"patches":[{"archive":"kanmi_cn.int","scene_encoding":"gbk"}]}\n'

# NewDES substitution table and inverse keyed round order.
ROTOR = bytes((
    32, 137, 239, 188, 102, 125, 221, 72, 212, 68, 81, 37, 86, 237, 147, 149,
    70, 229, 17, 124, 115, 207, 33, 20, 122, 143, 25, 215, 51, 183, 138, 142,
    146, 211, 110, 173, 1, 228, 189, 14, 103, 78, 162, 36, 253, 167, 116, 255,
    158, 45, 185, 50, 98, 168, 250, 235, 54, 141, 195, 247, 240, 63, 148, 2,
    224, 169, 214, 180, 62, 22, 117, 108, 19, 172, 161, 159, 160, 47, 43, 171,
    194, 175, 178, 56, 196, 112, 23, 220, 89, 21, 164, 130, 157, 8, 85, 251,
    216, 44, 94, 179, 226, 38, 90, 119, 40, 202, 34, 206, 35, 69, 231, 246,
    29, 109, 74, 71, 176, 6, 60, 145, 65, 13, 77, 151, 12, 127, 95, 199,
    57, 101, 5, 232, 150, 210, 129, 24, 181, 10, 121, 187, 48, 193, 139, 252,
    219, 64, 88, 233, 96, 128, 80, 53, 191, 144, 218, 11, 106, 132, 155, 104,
    91, 136, 31, 42, 243, 66, 126, 135, 30, 26, 87, 186, 182, 154, 242, 123,
    82, 166, 208, 39, 152, 190, 113, 205, 114, 105, 225, 84, 73, 163, 99, 111,
    204, 61, 200, 217, 170, 15, 198, 28, 192, 254, 134, 234, 222, 7, 236, 248,
    201, 41, 177, 156, 92, 131, 67, 249, 245, 184, 203, 9, 241, 0, 27, 46,
    133, 174, 75, 18, 93, 209, 100, 120, 76, 213, 16, 83, 4, 107, 140, 52,
    58, 55, 3, 244, 97, 197, 238, 227, 118, 49, 79, 230, 223, 165, 153, 59,
))


def decrypt(data: bytes, key: bytes) -> bytes:
    if len(data) % 8:
        raise ValueError('unaligned MoleBox segment')
    previous = bytes(8)
    decoded = bytearray()
    for offset in range(0, len(data), 8):
        block = data[offset:offset + 8]
        a, b, c, d, e, f, g, h = block
        for index in range(56, 0, -7):
            e ^= ROTOR[a ^ key[index % 15]]
            f ^= ROTOR[b ^ key[(index + 1) % 15]]
            g ^= ROTOR[c ^ key[(index + 2) % 15]]
            h ^= ROTOR[d ^ key[(index + 3) % 15]]
            b ^= ROTOR[e ^ key[(index - 3) % 15]]
            c ^= ROTOR[e ^ f]
            d ^= ROTOR[g ^ key[(index - 2) % 15]]
            a ^= ROTOR[h ^ key[(index - 1) % 15]]
        e ^= ROTOR[a ^ key[0]]
        f ^= ROTOR[b ^ key[1]]
        g ^= ROTOR[c ^ key[2]]
        h ^= ROTOR[d ^ key[3]]
        decoded.extend(x ^ y for x, y in zip((a, b, c, d, e, f, g, h), previous))
        previous = block
    return bytes(decoded)


def catalog(data: bytes) -> bytes:
    size, checksum = unpack_from('<II', decrypt(data[-8:], CATALOG_KEY))
    if not 0 < size <= 65536 or size > len(data) - 8:
        raise ValueError(f'invalid MoleBox catalog size: {size}')
    directory = decrypt(data[-8 - size:-8], CATALOG_KEY)
    if crc32(directory) != checksum:
        raise ValueError('MoleBox catalog CRC mismatch')
    return directory


def crc16(data: bytes) -> int:
    checksum = 0
    for byte in data:
        checksum ^= byte
        for _ in range(8):
            checksum = (checksum >> 1) ^ (0xA001 if checksum & 1 else 0)
    return checksum


def localized_scenes(data: bytes) -> dict[str, bytes]:
    directory = catalog(data)
    packages, package_count, mounts, mount_count = unpack_from('<4I', directory)
    if package_count != 1 or mount_count != 1:
        raise ValueError('expected one embedded package and one mount')
    header_offset, = unpack_from('<I', directory, packages)
    key = directory[mounts + 8:mounts + 24]
    header = decrypt(data[header_offset:header_offset + 48], key)
    tree_offset, tree_size, root, base_distance, checksum = unpack_from('<5I', header)
    if tree_offset + tree_size != header_offset:
        raise ValueError('invalid MoleBox tree location')
    tree = decrypt(data[tree_offset:header_offset], key)
    if crc32(tree) != checksum:
        raise ValueError('MoleBox tree CRC mismatch')

    def entry(offset):
        name_at, units, flags, size, count = unpack_from('<2I24x3I', tree, offset + 8)
        name = tree[name_at:name_at + 2 * units].decode('utf-16le')
        return name, flags, size, count

    scene_children = []
    _, _, _, root_children = entry(root)
    for (child,) in iter_unpack('<I', tree[root + 52:root + 52 + 4 * root_children]):
        name, flags, _, count = entry(child)
        if name == 'scene' and flags & 0x80000000:
            scene_children = list(iter_unpack('<I', tree[child + 52:child + 52 + 4 * count]))

    base = header_offset - base_distance
    scenes = {}
    for (child,) in scene_children:
        name, flags, size, count = entry(child)
        if not re.fullmatch(r'[a-z0-9_]+\.cst', name) or name in scenes or flags & 0x80000000:
            raise ValueError(f'unexpected scene entry {name!r}')
        content = bytearray()
        for index in range(count):
            packed, raw_size, block_flags, checksum, offset = unpack_from('<4HI', tree, child + 52 + 12 * index)
            start = base + offset
            if not base <= start < start + packed <= tree_offset or block_flags & ~6:
                raise ValueError(f'{name}: invalid MoleBox data block')
            block = data[start:start + packed]
            if block_flags & 4:
                block = decrypt(block, key)
            if block_flags & 2:
                decoder = decompressobj()
                block = decoder.decompress(block, raw_size + 1)
                if not decoder.eof or len(block) != raw_size:
                    raise ValueError(f'{name}: invalid compressed block')
            else:
                block = block[:raw_size]
            if len(block) != raw_size or crc16(block) != checksum:
                raise ValueError(f'{name}: block CRC mismatch')
            content.extend(block)
        if len(content) != size or content[:8] != b'CatScene':
            raise ValueError(f'{name}: invalid CatScene resource')
        scenes[name] = bytes(content)
    return scenes


def validate_inventory(scenes: dict[str, bytes], original: bytes) -> None:
    count, = unpack_from('<I', original, 4)
    expected = {name.split(b'\0', 1)[0].decode('ascii')
                for name, _, _ in iter_unpack('<64sII', original[8:8 + 72 * count])}
    if len(scenes) != 61 or scenes.keys() != expected:
        raise ValueError('the translated scenes do not match the original inventory')


def kif(files: dict[str, bytes]) -> bytes:
    directory = bytearray(b'KIF\0' + pack('<I', len(files)))
    offset = 8 + 72 * len(files)
    contents = []
    for name, content in sorted(files.items()):
        directory.extend(pack('<64sII', name.encode('ascii'), offset, len(content)))
        contents.append(content)
        offset += len(content)
    return bytes(directory) + b''.join(contents)


def replace_file(path: Path, data: bytes) -> None:
    with NamedTemporaryFile(dir=path.parent, delete=False) as handle:
        temporary = Path(handle.name)
        try:
            handle.write(data)
            handle.close()
            temporary.replace(path)
        finally:
            temporary.unlink(missing_ok=True)


def main() -> None:
    parser = ArgumentParser(description=__doc__)
    parser.add_argument('launcher', type=Path)
    parser.add_argument('original_scene', type=Path)
    parser.add_argument('patch_images', type=Path, help='image directory shipped with the Chinese patch')
    parser.add_argument('output', type=Path, help='directory for the player archive and manifest')
    args = parser.parse_args()
    data = args.launcher.read_bytes()
    if sha256(data).hexdigest() != LAUNCHER_SHA256:
        parser.error('the input is not the supported Kanmi Chinese patch launcher')
    original = args.original_scene.read_bytes()
    if sha256(original).hexdigest() != BASE_SCENE_SHA256:
        parser.error('the original scene archive is from an unsupported release')
    scenes = localized_scenes(data)
    validate_inventory(scenes, original)
    images = {path.name: path.read_bytes() for path in args.patch_images.glob('*.hg3') if path.is_file()}
    if (len(images) != 16 or any(not re.fullmatch(r'[a-z0-9_]+\.hg3', name)
                                or not content.startswith(b'HG-3') for name, content in images.items())):
        parser.error('expected 16 HG3 files from the Chinese patch')
    archive = kif(scenes | images)
    args.output.mkdir(parents=True, exist_ok=True)
    replace_file(args.output / ARCHIVE, archive)
    replace_file(args.output / 'cs2-localization.json', MANIFEST)
    print(f'{len(scenes)} scenes, {len(images)} images, SHA-256 {sha256(archive).hexdigest()}')


if __name__ == '__main__':
    main()
