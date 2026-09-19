#!/usr/bin/env python3
"""Render the User Book as a Word document.

    python3 scripts/build-user-book-docx.py [--output dist/user-book.docx]

Needs pandoc (3.x) and nothing else. The Markdown in `docs/user-book.md` stays
the single source the validators check; this script derives a `.docx` from it:

1. the title and subtitle move into document metadata and the Markdown table
   of contents is dropped in favour of a Word table of contents;
2. links that point at other files in the repository become GitHub URLs, since
   a Word document has no neighbouring files, while `#section` links stay
   internal and resolve to bookmarks pandoc generates with GitHub's slugs;
3. every pipe table's separator row is rewritten so column widths follow the
   content, the way pandoc sizes wide tables;
4. pandoc's default reference document is patched with a page-number footer,
   Letter pages, bordered tables with a shaded header row, and a smaller code
   face, then handed to pandoc as `--reference-doc`.

The output is a generated artifact and is not committed.
"""

from __future__ import annotations

import argparse
import datetime as dt
import re
import shutil
import subprocess
import sys
import tempfile
import zipfile
from pathlib import Path

REPOSITORY = Path(__file__).resolve().parent.parent
SOURCE = REPOSITORY / "docs" / "user-book.md"
GITHUB_BLOB = "https://github.com/makodb/mako-cloud/blob/main/"
TITLE = "The Mako Cloud User Book"
FENCE_LANGUAGES = {"ts": "typescript", "sh": "bash", "http": "", "text": ""}
MIN_DASHES, MAX_WEIGHT, WIDE_SEPARATOR = 3, 72, 96


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--output", default="dist/user-book.docx")
    parser.add_argument("--source", default=str(SOURCE))
    options = parser.parse_args()
    if shutil.which("pandoc") is None:
        print("pandoc is required: https://pandoc.org/installing.html", file=sys.stderr)
        return 1

    source = Path(options.source).read_text(encoding="utf-8")
    subtitle, body = preprocess(source)
    output = (REPOSITORY / options.output).resolve()
    output.parent.mkdir(parents=True, exist_ok=True)

    with tempfile.TemporaryDirectory(prefix="user-book-docx-") as scratch:
        scratch_path = Path(scratch)
        markdown = scratch_path / "user-book.md"
        markdown.write_text(body, encoding="utf-8")
        reference = scratch_path / "reference.docx"
        write_reference_document(reference)
        subprocess.run(
            [
                "pandoc",
                str(markdown),
                "--from",
                "markdown-raw_html+gfm_auto_identifiers",
                "--to",
                "docx",
                "--reference-doc",
                str(reference),
                "--shift-heading-level-by=-1",
                "--toc",
                "--toc-depth=2",
                "--highlight-style=tango",
                "--metadata",
                f"title={TITLE}",
                "--metadata",
                f"subtitle={subtitle}",
                "--metadata",
                f"date={dt.date.today():%B %Y}",
                "--metadata",
                "lang=en-US",
                "--output",
                str(output),
            ],
            check=True,
        )
    print(f"wrote {output.relative_to(REPOSITORY)} ({output.stat().st_size:,} bytes)")
    return 0


# ---- Markdown preprocessing -------------------------------------------------


def preprocess(source: str) -> tuple[str, str]:
    lines = source.split("\n")

    # The H1 and the italic line under it become the title block.
    assert lines[0].startswith("# "), "the book must start with its title"
    lines = lines[1:]
    subtitle = ""
    for index, line in enumerate(lines):
        if line.strip():
            if line.startswith("*") and line.endswith("*"):
                subtitle = line.strip("*").strip()
                lines = lines[index + 1 :]
            break

    # Word builds its own table of contents; drop the Markdown one and the
    # horizontal rules that separated chapters for a scrolling reader.
    text = "\n".join(lines)
    text = re.sub(r"\n## Table of contents\n.*?\n---\n", "\n", text, count=1, flags=re.S)
    text = re.sub(r"\n---\n", "\n", text)

    text = rewrite_links(text)
    text = rename_fences(text)
    text = size_tables(text)
    return subtitle, text


def rewrite_links(text: str) -> str:
    """Repository-relative link targets become GitHub URLs; `#section` links stay."""

    def replace(match: re.Match[str]) -> str:
        target = match.group(1)
        if target.startswith("#") or re.match(r"^[a-z]+:", target):
            return match.group(0)
        path, _, fragment = target.partition("#")
        if path.startswith("../"):
            path = path[3:]
        else:
            path = f"docs/{path}"
        url = GITHUB_BLOB + path + (f"#{fragment}" if fragment else "")
        return f"]({url})"

    return re.sub(r"\]\(([^)\s]+)\)", replace, text)


def rename_fences(text: str) -> str:
    """Map fence labels onto the names pandoc's highlighter knows."""

    def replace(match: re.Match[str]) -> str:
        language = match.group(1)
        return "```" + FENCE_LANGUAGES.get(language, language)

    return re.sub(r"^```([a-z]+)$", replace, text, flags=re.M)


def size_tables(text: str) -> str:
    """Rewrite each pipe table's separator so column widths follow content.

    Pandoc lays a table that is wider than its text column (72 characters by
    default) out across the full page and takes the relative column widths from
    the dashes in the separator row; a narrower table gets equal columns.
    Uniform `| --- |` separators would give a two-column command reference
    equal halves, so the dashes are content-weighted -- and the separator is
    scaled past the threshold so every table is laid out that way.
    """
    lines = text.split("\n")
    output: list[str] = []
    index = 0
    while index < len(lines):
        if not lines[index].startswith("|"):
            output.append(lines[index])
            index += 1
            continue
        block_start = index
        while index < len(lines) and lines[index].startswith("|"):
            index += 1
        block = lines[block_start:index]
        if len(block) >= 2 and re.match(r"^\|\s*:?-{3,}", block[1]):
            block[1] = separator_for(block)
        output.extend(block)
    return "\n".join(output)


def separator_for(block: list[str]) -> str:
    rows = [split_cells(line) for line in [block[0], *block[2:]]]
    columns = max(len(row) for row in rows)
    weights = []
    for column in range(columns):
        longest = max((len(row[column]) for row in rows if column < len(row)), default=0)
        weights.append(min(max(longest, MIN_DASHES), MAX_WEIGHT))
    total = sum(weights)
    if total < WIDE_SEPARATOR:
        weights = [max(MIN_DASHES, round(weight * WIDE_SEPARATOR / total)) for weight in weights]
    return "|" + "|".join(" " + "-" * weight + " " for weight in weights) + "|"


def split_cells(line: str) -> list[str]:
    inner = line.strip()
    if inner.startswith("|"):
        inner = inner[1:]
    if inner.endswith("|") and not inner.endswith("\\|"):
        inner = inner[:-1]
    return [cell.strip() for cell in re.split(r"(?<!\\)\|", inner)]


# ---- The reference document -------------------------------------------------

FOOTER_XML = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:ftr xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"
       xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
  <w:p>
    <w:pPr><w:pStyle w:val="Footer"/><w:jc w:val="center"/></w:pPr>
    <w:r><w:rPr><w:sz w:val="18"/></w:rPr><w:t xml:space="preserve">The Mako Cloud User Book · </w:t></w:r>
    <w:r><w:rPr><w:sz w:val="18"/></w:rPr><w:fldChar w:fldCharType="begin"/></w:r>
    <w:r><w:rPr><w:sz w:val="18"/></w:rPr><w:instrText xml:space="preserve"> PAGE </w:instrText></w:r>
    <w:r><w:rPr><w:sz w:val="18"/></w:rPr><w:fldChar w:fldCharType="separate"/></w:r>
    <w:r><w:rPr><w:sz w:val="18"/></w:rPr><w:t>1</w:t></w:r>
    <w:r><w:rPr><w:sz w:val="18"/></w:rPr><w:fldChar w:fldCharType="end"/></w:r>
  </w:p>
</w:ftr>
"""

SECTION_XML = (
    '<w:sectPr><w:footerReference w:type="default" r:id="rIdMakoFooter"/>'
    '<w:pgSz w:w="12240" w:h="15840"/>'
    '<w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440" '
    'w:header="720" w:footer="720" w:gutter="0"/></w:sectPr>'
)

TABLE_BORDERS = (
    '<w:tblBorders>'
    '<w:top w:val="single" w:sz="4" w:space="0" w:color="BFBFBF"/>'
    '<w:left w:val="single" w:sz="4" w:space="0" w:color="BFBFBF"/>'
    '<w:bottom w:val="single" w:sz="4" w:space="0" w:color="BFBFBF"/>'
    '<w:right w:val="single" w:sz="4" w:space="0" w:color="BFBFBF"/>'
    '<w:insideH w:val="single" w:sz="4" w:space="0" w:color="BFBFBF"/>'
    '<w:insideV w:val="single" w:sz="4" w:space="0" w:color="BFBFBF"/>'
    '</w:tblBorders>'
)

SOURCE_CODE_STYLE = (
    '<w:style w:type="paragraph" w:customStyle="1" w:styleId="SourceCode">'
    '<w:name w:val="Source Code"/><w:basedOn w:val="Normal"/><w:link w:val="VerbatimChar"/>'
    # Children of pPr are schema-ordered: borders, shading, wrapping, spacing.
    '<w:pPr><w:pBdr><w:top w:val="single" w:sz="4" w:space="4" w:color="E0E0E0"/>'
    '<w:left w:val="single" w:sz="4" w:space="4" w:color="E0E0E0"/>'
    '<w:bottom w:val="single" w:sz="4" w:space="4" w:color="E0E0E0"/>'
    '<w:right w:val="single" w:sz="4" w:space="4" w:color="E0E0E0"/></w:pBdr>'
    '<w:shd w:val="clear" w:color="auto" w:fill="F5F5F5"/>'
    '<w:wordWrap w:val="0"/><w:spacing w:before="60" w:after="60" w:line="240" w:lineRule="auto"/></w:pPr>'
    '<w:rPr><w:rFonts w:ascii="Consolas" w:hAnsi="Consolas"/><w:sz w:val="17"/><w:szCs w:val="17"/></w:rPr>'
    '</w:style>'
)

FOOTER_STYLE = (
    '<w:style w:type="paragraph" w:styleId="Footer"><w:name w:val="footer"/>'
    '<w:basedOn w:val="Normal"/><w:pPr><w:spacing w:before="0" w:after="0"/></w:pPr>'
    '<w:rPr><w:color w:val="7F7F7F"/></w:rPr></w:style>'
)


def write_reference_document(destination: Path) -> None:
    """Patch pandoc's own default reference document rather than ship a binary."""
    default = subprocess.run(
        ["pandoc", "--print-default-data-file", "reference.docx"],
        check=True,
        capture_output=True,
    ).stdout
    with tempfile.TemporaryDirectory(prefix="reference-") as scratch:
        original = Path(scratch) / "default.docx"
        original.write_bytes(default)
        with zipfile.ZipFile(original) as source, zipfile.ZipFile(
            destination, "w", zipfile.ZIP_DEFLATED
        ) as target:
            for item in source.infolist():
                data = source.read(item.filename)
                if item.filename == "word/styles.xml":
                    data = patch_styles(data.decode("utf-8")).encode("utf-8")
                elif item.filename == "word/document.xml":
                    data = patch_document(data.decode("utf-8")).encode("utf-8")
                elif item.filename == "word/_rels/document.xml.rels":
                    data = patch_relationships(data.decode("utf-8")).encode("utf-8")
                elif item.filename == "[Content_Types].xml":
                    data = patch_content_types(data.decode("utf-8")).encode("utf-8")
                target.writestr(item, data)
            target.writestr("word/footer1.xml", FOOTER_XML)


def patch_styles(xml: str) -> str:
    # 11 pt body text, 8.5 pt code, bordered tables with a shaded bold header row.
    xml = xml.replace(
        '<w:sz w:val="24" />\n        <w:szCs w:val="24" />',
        '<w:sz w:val="22" />\n        <w:szCs w:val="22" />',
        1,
    )
    xml = re.sub(
        r'(<w:style w:type="character" w:customStyle="1" w:styleId="VerbatimChar">.*?<w:sz w:val=")22(" />)',
        r"\g<1>17\g<2>",
        xml,
        count=1,
        flags=re.S,
    )
    table_style = re.search(r'<w:style w:type="table" w:default="1" w:styleId="Table">.*?</w:style>', xml, flags=re.S)
    assert table_style, "pandoc's reference document defines the Table style"
    patched = table_style.group(0)
    patched = patched.replace("<w:tblCellMar>", TABLE_BORDERS + "<w:tblCellMar>", 1)
    # The header row: bold runs (rPr precedes tblPr in a tblStylePr) and a shaded
    # cell (shd sits between tcBorders and vAlign in the existing tcPr).
    first_row = re.search(r'<w:tblStylePr w:type="firstRow">.*?</w:tblStylePr>', patched, flags=re.S)
    assert first_row, "the Table style carries a firstRow conditional format"
    header = first_row.group(0)
    header = header.replace("<w:tblPr>", "<w:rPr><w:b/><w:bCs/></w:rPr><w:tblPr>", 1)
    header = header.replace(
        "</w:tcBorders>", '</w:tcBorders><w:shd w:val="clear" w:color="auto" w:fill="EDEDED"/>', 1
    )
    patched = patched.replace(first_row.group(0), header, 1)
    xml = xml.replace(table_style.group(0), patched, 1)
    return xml.replace("</w:styles>", SOURCE_CODE_STYLE + FOOTER_STYLE + "</w:styles>", 1)


def patch_document(xml: str) -> str:
    # The default reference closes its body with an empty `<w:sectPr />`;
    # pandoc copies whatever section properties the reference carries.
    assert xml.count("<w:sectPr") == 1, "the default reference document carries one section"
    return re.sub(r"<w:sectPr\s*/>|<w:sectPr>.*?</w:sectPr>", SECTION_XML, xml, count=1, flags=re.S)


def patch_relationships(xml: str) -> str:
    relationship = (
        '<Relationship Id="rIdMakoFooter" '
        'Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/footer" '
        'Target="footer1.xml"/>'
    )
    return xml.replace("</Relationships>", relationship + "</Relationships>", 1)


def patch_content_types(xml: str) -> str:
    override = (
        '<Override PartName="/word/footer1.xml" '
        'ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.footer+xml"/>'
    )
    return xml.replace("</Types>", override + "</Types>", 1)


if __name__ == "__main__":
    sys.exit(main())
