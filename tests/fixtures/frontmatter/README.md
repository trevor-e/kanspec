The adversarial frontmatter corpus.

Every file here is a legal kanspec entity that some part of the write path could plausibly
corrupt: inline comments on the very line an edit lands on, standalone comments and the
blank lines around them, deliberate quoting (`"S"`), non-alphabetical key order, a
`severity_hint_v2` key written by a NEWER kanspec, unquoted globs in flow sequences, CRLF
line endings, a BOM, a file that ends without a newline, a body with trailing whitespace,
and a `---` inside the BODY (which must not be mistaken for a fence).

`tests/fm_bytes.rs` asserts, over every file: `split(src).render() == src` byte-for-byte,
that 100 round-tripping edits reproduce the file byte-identically, and that the only lines
whose bytes change are the ones the edit actually names.

Owner: **F** (the corpus) / **S1** (the assertions).
