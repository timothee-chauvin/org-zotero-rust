# org-zotero-rust

Like [org-readwise-rust](https://github.com/timothee-chauvin/org-readwise-rust), but for [Zotero](https://www.zotero.org/).

This doesn't use any API, but directly queries the Zotero sqlite3 database.

## Known issues
Papers in Zotero's trash are still included. Current solution: empty the trash.
