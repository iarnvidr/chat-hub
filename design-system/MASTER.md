# Chat Hub workspace

Use a compact, content-first desktop workspace. One conversation sidebar, a 42px tab strip, and one reading/live-chat canvas. No separate workspace navigation or introductory hero. Settings is a separate page that preserves the active chat and hides the native provider while open.

The UI/UX Pro Max design-system and accessibility searches informed this revision. Its generated newsletter/conversion pattern is unsuitable for an installed chat app; use the existing native system font and flat surfaces instead. Keep the established cyan accent for primary actions, with semantic surface/text/border tokens. No remote fonts or decorative assets.

- Sidebar: 272px desktop, 230px smaller windows; search and New chat at top, Settings at bottom. Conversation rows are compact, with full title available on hover.
- Canvas: local history by default; individual conversations use tabs. Live provider chrome stays minimal. Connection, sync, capture, imports, backups, updates and service details belong in Settings.
- Feedback: background sync uses a short sidebar status and detailed Settings status. Avoid persistent progress overlays on the chat.
- Interaction: visible keyboard focus, labeled icons, predictable Back/Escape, preserved drafts/selection, no animation-dependent access. Respect reduced motion.
- Responsive: below 650px show the list or active chat; Back restores the list. Settings remains reachable from the list.
- Verify keyboard navigation, contrast, overflow, native child-view bounds, Settings suspension/restoration, and existing import/restore/update flows.
