"""Render shared, reviewed legal content for the React and static edge sites."""
import argparse
import html
import json
import hashlib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def outputs():
    data = json.loads((ROOT / "legal/documents.json").read_text(encoding="utf-8"))
    result = {ROOT / "website/src/lib/legal-documents.json": json.dumps(data, ensure_ascii=False, indent=2) + "\n"}
    for slug in ("privacy-policy", "terms-of-service", "third-party-licenses"):
        document = data[slug]
        sections = []
        for section in document["sections"]:
            paragraphs = "".join(f"<p>{html.escape(text)}</p>" for text in section["paragraphs"])
            links = "".join(f'<li><a href="{html.escape(link["href"], quote=True)}">{html.escape(link["label"])}</a></li>' for link in section.get("links", []))
            sections.append(f'<section><h2>{html.escape(section["title"])}</h2>{paragraphs}' + (f"<ul>{links}</ul>" if links else "") + "</section>")
        page = f'''<!doctype html>
<!-- Generated from legal/documents.json by scripts/sync-legal-documents.py. -->
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<title>{html.escape(document['title'])} | Atlas</title><meta name="description" content="{html.escape(document['description'], quote=True)}">
<link rel="canonical" href="https://atlasnative.com/{slug}"><link rel="icon" href="/favicon.ico" sizes="48x48"><link rel="icon" type="image/svg+xml" href="/favicon.svg"><link rel="icon" type="image/png" sizes="96x96" href="/assets/favicon-96.png"><link rel="icon" type="image/png" sizes="192x192" href="/assets/icon-192.png"><link rel="apple-touch-icon" href="/apple-touch-icon.png"><link rel="manifest" href="/site.webmanifest"><link rel="stylesheet" href="/styles.css">
<style>.prose section{{margin-block:2rem}}.prose p{{margin-block:1rem;line-height:1.8}}.prose a{{text-decoration:underline;text-underline-offset:3px}}.prose h2{{line-height:1.3}}.prose ul{{padding-left:1.5rem}}.legal-nav{{display:flex;gap:1rem;flex-wrap:wrap}}</style>
<script defer src="/privacy-choices.js"></script></head>
<body class="doc"><header class="nav"><a class="brand" href="/"><img src="/assets/atlas-logo.png" alt="" width="28" height="28"><span>Atlas</span></a><a class="btn light sm" href="/">Back to Atlas</a></header>
<main class="prose"><h1>{html.escape(document['title'])}</h1><p>Last updated: <time datetime="{data['updated']}">{data['updated']}</time></p>{''.join(sections)}</main>
<footer class="footer"><div class="bottom"><span>© 2026 Atlas.</span><nav aria-label="Legal" class="legal-nav"><a href="/privacy-policy">Privacy</a><a href="/terms-of-service">Terms</a><a href="/third-party-licenses">Third-party licenses</a><a href="mailto:support@modhyt.org">Support</a><a href="https://icons8.com">Icons8</a><button type="button" data-privacy-choices>Privacy choices</button></nav></div></footer></body></html>
'''
        result[ROOT / "website-edge/public" / f"{slug}.html"] = page
        route = f'''// Generated from legal/documents.json by scripts/sync-legal-documents.py.
import type {{ Metadata }} from "next";
import LegalDocument from "@/components/LegalDocument";
import documents from "@/lib/legal-documents.json";

export const metadata: Metadata = {{
  title: documents["{slug}"].title,
  description: documents["{slug}"].description,
  alternates: {{ canonical: "/{slug}" }},
}};

export default function Page() {{ return <LegalDocument slug="{slug}" />; }}
'''
        result[ROOT / "website/src/app" / slug / "page.tsx"] = route
    source = (ROOT / "legal/privacy-choices.js").read_text(encoding="utf-8")
    result[ROOT / "website/public/privacy-choices.js"] = source
    result[ROOT / "website-edge/public/privacy-choices.js"] = source
    checker = (ROOT / "legal/check-site.mjs").read_text(encoding="utf-8")
    rclone_notice_path = "packaging/windows/msix/RCLONE-THIRD-PARTY-NOTICES.txt"
    rclone_notices = (ROOT / rclone_notice_path).read_text(encoding="utf-8")
    # Update our web-only introduction; upstream copyright/license blocks remain intact.
    rclone_notices = rclone_notices.replace(
        "Source and the verification manifest are available without charge at:\nhttps://github.com/neondeex/atlas-filemanager",
        "For license and source questions, contact support@modhyt.org.",
    )
    for site in ("website", "website-edge"):
        result[ROOT / site / "public/legal/RCLONE-THIRD-PARTY-NOTICES.txt"] = rclone_notices
        result[ROOT / site / "scripts/check-legal.mjs"] = checker
        files = {path.relative_to(ROOT / site).as_posix(): hashlib.sha256(content.encode("utf-8")).hexdigest()
                 for path, content in result.items() if path.is_relative_to(ROOT / site)}
        files["public/legal/THIRD-PARTY-NOTICES.txt"] = hashlib.sha256((ROOT / "legal/THIRD-PARTY-NOTICES.txt").read_bytes()).hexdigest()
        result[ROOT / site / "public/legal/build-manifest.json"] = json.dumps({"updated":data["updated"], "files":files}, indent=2) + "\n"
    manifest = {
        "sources": {file: hashlib.sha256((ROOT / file).read_bytes()).hexdigest() for file in ("legal/documents.json", "legal/privacy-choices.js", "legal/check-site.mjs", rclone_notice_path)},
        "outputs": {path.relative_to(ROOT).as_posix(): hashlib.sha256(content.encode("utf-8")).hexdigest() for path, content in result.items()},
    }
    result[ROOT / "legal/generated-files.json"] = json.dumps(manifest, indent=2) + "\n"
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    stale = []
    for path, content in outputs().items():
        if args.check:
            if not path.is_file() or path.read_text(encoding="utf-8") != content:
                stale.append(str(path.relative_to(ROOT)))
        else:
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(content, encoding="utf-8", newline="\n")
    if stale:
        print("Stale legal documents: " + ", ".join(stale))
        raise SystemExit(1)
    print("Legal documents and privacy controls are synchronized for both sites.")


if __name__ == "__main__":
    main()
