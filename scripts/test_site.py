#!/usr/bin/env python3
"""Check the dependency-free Pages site's HTML structure and local links."""
from html.parser import HTMLParser
from pathlib import Path
from urllib.parse import urlsplit, unquote

ROOT = Path(__file__).resolve().parents[1]
SITE = ROOT / 'docs'
VOID = {'area', 'base', 'br', 'col', 'embed', 'hr', 'img', 'input', 'link', 'meta', 'param', 'source', 'track', 'wbr'}

class Page(HTMLParser):
    def __init__(self, path):
        super().__init__(convert_charrefs=True)
        self.path, self.ids, self.links, self.stack = path, set(), [], []
        self.h1 = 0
        self.feed(path.read_text())
        self.close()
        assert not self.stack, (path, self.stack)
        assert self.h1 == 1, (path, 'Expected one h1')

    def handle_starttag(self, tag, attrs):
        attrs = dict(attrs)
        if tag == 'h1': self.h1 += 1
        if 'id' in attrs:
            assert attrs['id'] not in self.ids, (self.path, 'Duplicate ID', attrs['id'])
            self.ids.add(attrs['id'])
        for name in ('href', 'src'):
            if name in attrs: self.links.append(attrs[name])
        if tag not in VOID: self.stack.append(tag)

    def handle_endtag(self, tag):
        assert self.stack and self.stack.pop() == tag, (self.path, tag)

pages = {path.resolve(): Page(path) for path in SITE.glob('*.html')}
for path, page in pages.items():
    for link in page.links:
        url = urlsplit(link)
        if url.scheme or url.netloc: continue
        target = (path.parent / unquote(url.path)).resolve() if url.path else path
        if target.is_dir(): target /= 'index.html'
        if target == SITE / 'install.sh':
            # Pages copies the canonical script at deployment time.
            assert (ROOT / 'scripts/install.sh').is_file()
            continue
        assert target.is_file(), (path, link, 'Missing local file')
        if url.fragment:
            assert target in pages and unquote(url.fragment) in pages[target].ids, (path, link, 'Missing anchor')
print(f'Site: {len(pages)} pages; HTML nesting, headings, IDs, files and local anchors passed')
