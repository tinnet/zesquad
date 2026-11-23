# Vendored static files

Compiled into the binary with rust-embed; nothing here is fetched at runtime.

| File | Source |
|------|--------|
| `htmx-2.0.11.min.js` | <https://unpkg.com/htmx.org@2.0.11/dist/htmx.min.js> (htmx 2.0.11, BSD-2-Clause) |
| `style.css`, `share.js`, `slow-down.js` | ours |

To update htmx, download the new version under its versioned name and change the `<script>`
in `templates/base.html`.
