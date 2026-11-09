# Merge previews and obsolete drafts

A preview has three identities, deliberately kept separate:

* The embedded application/base version (currently 0.2.5). This is not automatically bumped on merges.
* The preview release identity, such as `0.2.5-preview.e8120a4e8267`. GitHub titles, ZIP/DMG filenames and `build-info.json` use this unique identity. The tag remains `merge-<full accepted SHA>`.
* The native source commit/tree, visible under Settings → Software. These are embedded at compilation. When acceptance reuses a PR artifact with an identical source tree, its source commit can precede the accepted merge. Do not relabel the embedded source as the merge commit.

Publication copies accepted ZIP/DMG bytes unchanged. It does not rebuild, re-sign, alter the stable updater, overwrite existing assets or move `releases/latest`. SHA256SUMS uses the unique uploaded filenames. `build-info.json` schema 2 explicitly declares `channel: merge-preview`, `app_version` and `display_version`.

A published prerelease is not a draft. Existing historical releases are not rewritten by this change. Backfilling acceptance runs from commits predating this helper should use their original publisher, not relabel or overwrite their immutable assets.

## Obsolete empty draft cleanup

The old `v0.2.0` draft is unrelated to automatic previews. After checking it is no longer needed for an active review, run **Audit or remove an obsolete empty release draft** on `main`, supplying the exact numeric release ID and tag. The default is audit-only. Set `delete` only after reviewing that result.

The workflow refuses published releases, drafts with uploaded assets, tag/ID mismatches and versions equal to or newer than the latest published stable. It never deletes Git tags. A draft with assets needs an explicit manual review rather than this cleanup path.

## Validation

Run `python3 -m unittest discover -s build -p 'test_merge_preview_identity.py'` and `node --test tests/build_identity.test.cjs`. Normal hosted native acceptance must still compile the build script and confirm the generated identity is embedded. After merge, verify the new SHA-specific assets and manifest, and that the stable release remains unchanged.
