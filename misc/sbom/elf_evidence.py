"""Compare loader-visible ELF content while allowing non-runtime stripping changes."""

import hashlib
import json
import struct


def elf_runtime(path):
    try:
        return _elf_runtime(path)
    except (struct.error, UnicodeError) as error:
        raise ValueError("Invalid/truncated ELF metadata") from error


def _elf_runtime(path):
    with path.open("rb") as stream:
        ident = stream.read(16)
        if len(ident) != 16 or ident[:4] != b"\x7fELF" or ident[4] not in {1, 2} or ident[5] not in {1, 2}:
            raise ValueError(f"Final-byte verification requires a supported ELF file: {path}")
        endian = "<" if ident[5] == 1 else ">"
        wide = ident[4] == 2
        header_format = endian + ("HHIQQQIHHHHHH" if wide else "HHIIIIIHHHHHH")
        header = struct.unpack(header_format, stream.read(struct.calcsize(header_format)))
        kind, machine, version, entry, phoff, shoff, flags, ehsize, phsize, phnum, shsize, shnum, names_index = header
        if not shnum or names_index >= shnum or not phnum or phnum == 65535:
            raise ValueError("Missing or unsupported extended ELF section/program headers")
        section_format = endian + ("IIQQQQIIQQ" if wide else "IIIIIIIIII")
        program_format = endian + ("IIQQQQQQ" if wide else "IIIIIIII")
        if (ehsize != 16 + struct.calcsize(header_format) or ident[6] != 1 or version != 1 or
                shsize != struct.calcsize(section_format) or phsize != struct.calcsize(program_format)):
            raise ValueError("Unsupported ELF table entry sizes")
        stream.seek(0, 2)
        size = stream.tell()

        def data(offset, length):
            if offset < 0 or length < 0 or offset + length > size:
                raise ValueError("ELF content is outside the file")
            stream.seek(offset)
            value = stream.read(length)
            if len(value) != length:
                raise ValueError("Truncated ELF content")
            return value

        sections = [struct.unpack(section_format, data(shoff + number * shsize, shsize)) for number in range(shnum)]
        names = sections[names_index]
        strings = data(names[4], names[5])
        allocated = []
        audit = []
        seen = set()
        for name_offset, section_type, section_flags, address, offset, length, _, _, alignment, _ in sections:
            if name_offset >= len(strings):
                raise ValueError("Invalid ELF section name offset")
            end = strings.find(b"\0", name_offset)
            if end < 0:
                raise ValueError("Unterminated ELF section name")
            name = strings[name_offset:end].decode("utf-8", errors="strict")
            if section_flags & 2 or name == ".dep-v0":
                if name in seen:
                    raise ValueError("Ambiguous duplicate runtime/audit section")
                seen.add(name)
                checksum = None if section_type == 8 else hashlib.sha256(data(offset, length)).hexdigest()
                record = {"name": name, "type": section_type, "flags": section_flags, "address": address,
                          "size": length, "alignment": alignment, "sha256": checksum}
                if section_flags & 2:
                    record["offset"] = offset
                    allocated.append(record)
                if name == ".dep-v0":
                    audit.append(record)
        if not allocated or not any(section["name"] == ".text" for section in allocated):
            raise ValueError("ELF file has no inspectable runtime text")
        programs = []
        # Stripping can relocate/reduce the section table, which the loader does
        # not use. No other ELF-header or program-table bytes may change.
        metadata_ranges = ((40, 48), (60, 64)) if wide else ((32, 36), (48, 52))
        for number in range(phnum):
            record = struct.unpack(program_format, data(phoff + number * phsize, phsize))
            if wide:
                program_type, program_flags, offset, virtual, physical, file_size, memory_size, alignment = record
            else:
                program_type, offset, virtual, physical, file_size, memory_size, program_flags, alignment = record
            if program_type == 1 and (file_size > memory_size or
                    alignment > 1 and (alignment & (alignment - 1) or offset % alignment != virtual % alignment)):
                raise ValueError("Invalid ELF load segment")
            payload = bytearray(data(offset, file_size))
            for start, end in metadata_ranges:
                left, right = max(start, offset), min(end, offset + file_size)
                if left < right:
                    payload[left - offset:right - offset] = bytes(right - left)
            programs.append({"type": program_type, "flags": program_flags, "offset": offset,
                             "virtual": virtual, "physical": physical, "file_size": file_size,
                             "memory_size": memory_size, "alignment": alignment,
                             "sha256": hashlib.sha256(payload).hexdigest()})
        if not any(program["type"] == 1 for program in programs):
            raise ValueError("ELF file has no load segments")
        return {"identity": ident.hex(), "type": kind, "machine": machine, "version": version,
                "entry": entry, "flags": flags, "header_size": ehsize, "program_offset": phoff,
                "program_entry_size": phsize, "section_entry_size": shsize, "programs": programs,
                "allocated_sections": allocated, "embedded_audit": audit}


def verify_postprocessing(original, final):
    before, after = elf_runtime(original), elf_runtime(final)
    if not before["embedded_audit"]:
        raise ValueError("Original artifact lacks embedded Cargo audit metadata")
    if before != after:
        raise ValueError("Final artifact changed loader-visible ELF content or embedded audit metadata")
    return {"method": "elf-mapped-segments-and-embedded-audit-equivalence-v2",
            "runtime_sha256": hashlib.sha256(json.dumps(before, sort_keys=True, separators=(",", ":")).encode()).hexdigest(),
            "allocated_sections": len(before["allocated_sections"]),
            "permitted_header_changes": ["e_shoff", "e_shnum", "e_shstrndx"],
            "limitation": "Trusted-builder byte equivalence, not independent source reproducibility or exhaustive native identities."}
