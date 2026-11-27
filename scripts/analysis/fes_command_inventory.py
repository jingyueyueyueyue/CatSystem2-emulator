"""Inventory FES statements and initialization fields across game resources."""

import argparse
from collections import Counter, defaultdict
import hashlib
import json
from pathlib import Path
import re
import struct
import zlib

import pefile

from audit_runtime_evidence import blowfish_le_decrypt, runtime_fingerprint
from startup_inventory import entries_at, normalized


ROOT = Path(__file__).resolve().parents[2]
DECLARATIONS = {b'INCLUDE', b'PRELOAD', b'MACRO', b'DEFINE', b'OBJECT', b'KEYBLOCK', b'SUBCOMMAND'}
EXCLUDED = {'backup', 'backups', 'save', 'savedata', 'patch', 'patches', '\u8865\u4e01', '\u5907\u4efd'}
ARCHIVE_SUFFIXES = {'.int', '.dat', '.kif'}
COMMON_EXTENSIONS = {b'.fes', b'.cst', b'.cstl', b'.kcs', b'.anm', b'.hg3', b'.hg2', b'.xml', b'.ogg', b'.wav', b'.pcm', b'.txt', b'.dat', b'.psz', b'.wmv', b'.kcs2', b'.png', b'.jpg', b'.img', b'.mot', b'.cdb', b'.ptc', b'.ppi'}
IDENTIFIER = re.compile(rb'[a-zA-Z_][a-zA-Z_0-9.]*\Z')
MACRO_NAME = re.compile(rb'(?<![A-Za-z_0-9])(?:[A-Z_][A-Z_0-9]*)\b')
COMMENT = re.compile(rb'/\*.*?\*/|//[^\r\n]*|"(?:[^"\\]|\\.)*"|\'(?:[^\'\\]|\\.)*\'', re.S)
CP932_PAIR = re.compile(rb'[\x81-\x9f\xe0-\xfc][\x40-\x7e\x80-\xfc]')


def text(value):
    return value.decode('cp932', 'backslashreplace')


def clean(data):
    # Align line numbers and protect multibyte trail bytes from the lexer.
    data = CP932_PAIR.sub(lambda match: b'\xff\xff', data)
    return COMMENT.sub(lambda match: re.sub(rb'[^\r\n]', b' ', match[0])
                       if match[0].startswith((b'//', b'/*')) else match[0], data)


def original_lines(data):
    for number, (cleaned, original) in enumerate(zip(clean(data).split(b'\n'), data.split(b'\n')), 1):
        restored = bytes(old if new == 0xff else new for old, new in zip(original, cleaned))
        if line := restored.strip():
            yield number, line


def unpack(data):
    if not data.startswith(b'FES\0'):
        return data
    packed, length = struct.unpack_from('<II', data, 4)
    if len(data) != 16 + packed:
        raise ValueError('FES packed size mismatch')
    output = zlib.decompress(data[16:])
    if len(output) != length:
        raise ValueError('FES expanded size mismatch')
    return output


def name_base(token):
    return token.split(b'[', 1)[0]


def statement(line, macros, kind):
    while line.startswith((b'if ', b'if(')):
        # Inline IF may contain nested input expressions before the command.
        first = line.find(b'(')
        if first < 0:
            return 'global:if'
        depth, quote = 0, None
        for i in range(first, len(line)):
            byte = line[i]
            if quote is not None:
                if byte == quote:
                    quote = None
            elif byte in (34, 39):
                quote = byte
            elif byte == 40:
                depth += 1
            elif byte == 41:
                depth -= 1
                if depth == 0:
                    break
        if depth != 0 or not (line := line[i + 1:].strip()):
            return 'global:if'
    line = MACRO_NAME.sub(lambda match: macros.get(match[0], match[0]), line)
    parts = line.split(maxsplit=2)
    if not parts:
        return 'unclassified'
    target = name_base(parts[0])
    if target in kind or target == b'@':
        if len(parts) < 2 or not IDENTIFIER.fullmatch(parts[1]):
            return 'expression'
        return text(kind.get(target, b'@')) + ':' + text(parts[1])
    if IDENTIFIER.fullmatch(target) and target == parts[0]:
        if len(parts) > 1 and re.match(rb'(?:[-+*/%]|[<>!&|^])?=|[+-]{2}', parts[1]):
            return 'expression'
        return 'global:' + text(target)
    if parts[0].startswith((b'\\', b'$', b'(', b'++', b'--')):
        return 'expression'
    return 'unclassified'


def script_sections(body):
    sections = []
    for number, line in original_lines(body):
        if line.startswith(b'#'):
            fields = line[1:].split()
            if fields:
                sections.append((fields, []))
        elif sections:
            sections[-1][1].append((number, line))
    return sections


def script_subcommands(sections):
    bindings = {}
    for fields, lines in sections:
        if fields[0] != b'SUBCOMMAND':
            continue
        for number, line in lines:
            words = line.split()
            if (len(words) >= 4 and words[0] in (b'oneshot', b'continue')
                    and b':' in words[3:] and IDENTIFIER.fullmatch(words[1])):
                bindings.setdefault(text(words[1]), {'target': text(words[2]), 'line': number})
    return bindings


def script_commands(sections):
    kinds, macros = {}, {}
    for fields, lines in sections:
        if fields[0] not in (b'DEFINE', b'MACRO'):
            continue
        for _, line in lines:
            words = line.split(maxsplit=1)
            if len(words) < 2:
                continue
            if fields[0] == b'DEFINE':
                kinds[name_base(words[1].split()[0])] = words[0]
            else:
                value = words[1]
                if value.startswith(b'"') and b'"' in value[1:]:
                    value = value[1:].split(b'"', 1)[0]
                else:
                    value = value.split()[0]
                macros[words[0]] = value

    for fields, lines in sections:
        header = fields[0]
        for number, line in lines:
            if header == b'OBJECT':
                target = name_base(line.split()[0])
                for field in fields[1:]:
                    yield ('init/' + text(kinds.get(target, b'?')) + ':' + text(field)), number, line
            elif header in DECLARATIONS:
                if header == b'DEFINE':
                    yield ('define:' + text(line.split()[0])), number, line
            else:
                yield statement(line, macros, kinds), number, line


def archive_entries(path, seeds):
    with path.open('rb') as source:
        if source.read(4) != b'KIF\0':
            return None
    best = None
    for seed in seeds:
        entries, _, cipher = entries_at(path, seed)
        if not cipher or len(seeds) == 1:
            return entries, cipher
        # Ciphertext bytes can yield plausible paths under the wrong V_CODE.
        score = sum(b'.' + entry['name'].lower().rsplit(b'.', 1)[-1] in COMMON_EXTENSIONS
                    for entry in entries)
        if best is None or score > best[0]:
            best = score, entries, cipher
    return best[1:]


def game_files(root):
    return sorted(path for path in root.rglob('*') if path.is_file()
                  and not any(part.lower() in EXCLUDED for part in path.relative_to(root).parts[:-1]))


def audit_game(root, selected_seeds):
    game = {'path': str(root), 'scripts': [], 'diagnostics': [], 'archives_indexed': 0, 'archives_other_format': []}
    counts, examples = Counter(), defaultdict(list)
    files = game_files(root)
    known_seeds = set(selected_seeds.values())
    if not known_seeds:
        for path in files:
            if path.suffix.lower() != '.exe':
                continue
            try:
                known_seeds.add(int(runtime_fingerprint(path)['archive_name_seed'], 16))
            except (ValueError, OSError, pefile.PEFormatError):
                continue

    def add(source, resource, content):
        try:
            body = unpack(content)
            if not content.startswith(b'FES\0') and not re.search(rb'^\s*#[A-Za-z_]', body, re.M):
                raise ValueError('no FES signature or structured text headers')
            sections = script_sections(body)
            statements = Counter()
            for key, number, line in script_commands(sections):
                statements[key] += 1
                if len(examples[key]) < 4:
                    examples[key].append({'source': source, 'line': number, 'statement': text(line)})
            counts.update(statements)
            game['scripts'].append({'source': source, 'resource': resource, 'sha256': hashlib.sha256(content).hexdigest(),
                                    'statements': dict(sorted(statements.items())),
                                    'subcommands': script_subcommands(sections)})
        except (ValueError, zlib.error, struct.error) as error:
            game['diagnostics'].append({'source': source, 'error': str(error)})

    for path in files:
        source = path.relative_to(root).as_posix()
        suffix = path.suffix.lower()
        try:
            if suffix == '.fes':
                add(source, normalized(source), path.read_bytes())
            elif suffix in ARCHIVE_SUFFIXES:
                seeds = [selected_seeds[normalized(source)]] if normalized(source) in selected_seeds else sorted(known_seeds) or [0]
                archive = archive_entries(path, seeds)
                if not archive:
                    if suffix in ('.int', '.kif'):
                        game['archives_other_format'].append(source)
                    continue
                game['archives_indexed'] += 1
                entries, cipher = archive
                with path.open('rb') as handle:
                    for entry in entries:
                        if not entry['name'].lower().endswith(b'.fes'):
                            continue
                        name = text(entry['name']).replace('\\', '/')
                        handle.seek(entry['offset'])
                        data = handle.read(entry['size'])
                        add(source + '::' + name, normalized(name), blowfish_le_decrypt(cipher, data) if cipher else data)
        except (OSError, ValueError, struct.error) as error:
            game['diagnostics'].append({'source': source, 'error': str(error)})
    game['totals'] = dict(sorted(counts.items()))
    game['examples'] = dict(sorted(examples.items()))
    return game


def main():
    args = argparse.ArgumentParser(description=__doc__)
    args.add_argument('--output', required=True, type=Path)
    options = args.parse_args()
    audit = json.loads((ROOT / 'reports/all_sample_content_audit.json').read_text(encoding='utf-8'))
    seed_maps = {Path(game['root']).name: {normalized(name): int(value.split()[0], 0)
                 for name, value in game['archives']['selected_name_seeds'].items()} for game in audit['games']}
    games = []
    for root in sorted((ROOT / 'games').iterdir()):
        if root.is_dir():
            game = audit_game(root, seed_maps.get(root.name, {}))
            games.append(game)
            print(f"{root.name}: {len(game['scripts'])} FES, {game['archives_indexed']} KIF, "
                  f"{len(game['archives_other_format'])} other archives, {len(game['diagnostics'])} diagnostics", flush=True)

    inventory = {'schema': 'cs2-fes-command-inventory/v1', 'game_count': len(games),
                 'script_count': sum(len(game['scripts']) for game in games), 'games': games}
    options.output.write_text(json.dumps(inventory, ensure_ascii=False, indent=2) + '\n', encoding='utf-8')


if __name__ == '__main__':
    main()
