#!/usr/bin/env python3
"""A PDF/X-4 preflight for Quill's print PDFs (#1303), with only the standard
library: the checks KDP and IngramSpark run that a file can fail on its own.

    scripts/pdfx-preflight.py FILE.print.pdf [FILE.cover.pdf ...]

For each file: a PDF 1.6 header; /GTS_PDFXVersion (PDF/X-4) in /Info and the
XMP; a /GTS_PDFX output intent with an embedded ICC profile; /Trapped and the
dates in /Info; a trailer /ID; a TrimBox and BleedBox on every page, the trim
inside the bleed inside the media box; every font embedded; and no uncalibrated
RGB. Exits 1 and says which check failed when any does.
"""
import re
import sys


def boxes(page):
    found = {}
    for name in ("MediaBox", "TrimBox", "BleedBox"):
        m = re.search(rb"/" + name.encode() + rb" \[([^\]]+)\]", page)
        if m:
            found[name] = [float(v) for v in m.group(1).split()]
    return found


def inside(inner, outer):
    return (inner[0] >= outer[0] - 0.01 and inner[1] >= outer[1] - 0.01
            and inner[2] <= outer[2] + 0.01 and inner[3] <= outer[3] + 0.01)


def check(path):
    data = open(path, "rb").read()
    problems = []

    def need(ok, what):
        if not ok:
            problems.append(what)

    need(data.startswith(b"%PDF-1.6"), "header isn't PDF 1.6")
    need(b"/GTS_PDFXVersion (PDF/X-4)" in data, "/Info has no /GTS_PDFXVersion (PDF/X-4)")
    need(b"<pdfxid:GTS_PDFXVersion>PDF/X-4</pdfxid:GTS_PDFXVersion>" in data,
         "the XMP doesn't name PDF/X-4")
    need(b"/Type /Metadata /Subtype /XML" in data, "no XMP metadata stream")
    need(b"/S /GTS_PDFX" in data and b"/DestOutputProfile" in data,
         "no PDF/X output intent with an ICC profile")
    need(b"/Trapped /False" in data or b"/Trapped /True" in data, "no /Trapped in /Info")
    need(b"/CreationDate (D:" in data and b"/ModDate (D:" in data, "no dates in /Info")
    need(re.search(rb"trailer\s*<<[^>]*/ID \[<", data) is not None, "no trailer /ID")
    need(b"/DeviceRGB" not in data, "uncalibrated RGB")

    # Every page: a trim inside a bleed inside the media box. The media box may
    # be inherited from the page tree.
    inherited = boxes(re.search(rb"/Type /Pages.*?>>", data, re.S).group(0)) if b"/Type /Pages" in data else {}
    pages = re.findall(rb"<< /Type /Page /Parent[^\n]*?>>", data)
    need(pages, "no pages")
    for i, page in enumerate(pages, 1):
        b = {**inherited, **boxes(page)}
        if "TrimBox" not in b or "BleedBox" not in b or "MediaBox" not in b:
            problems.append(f"page {i} lacks a TrimBox, BleedBox, or MediaBox")
            break
        if not (inside(b["TrimBox"], b["BleedBox"]) and inside(b["BleedBox"], b["MediaBox"])):
            problems.append(f"page {i}: the trim isn't inside the bleed inside the media box")
            break

    # Every font embedded: each font descriptor carries a font file.
    for desc in re.findall(rb"<< /Type /FontDescriptor.*?>>", data, re.S):
        if not re.search(rb"/FontFile[23]? ", desc):
            name = re.search(rb"/FontName /(\S+)", desc)
            problems.append(f"font {name.group(1).decode() if name else '?'} isn't embedded")

    return problems


failed = False
for path in sys.argv[1:]:
    problems = check(path)
    print(f"== PDF/X-4: {path.rsplit('/', 1)[-1]}: {'ok' if not problems else 'FAILED'}")
    for problem in problems:
        print(f"  {problem}")
    failed = failed or bool(problems)
sys.exit(1 if failed else 0)
