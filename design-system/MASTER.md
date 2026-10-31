# Chat Hub — classified terminal

The workspace borrows its visual language from Muninn and the esbenthomsen.com classified-terminal design reference: dark graphite surfaces, warm amber as the single interface accent, ivory reading text, compact machine labels, flat borders, and square corners. Keep the chat readable as a transcript, not as a dashboard.

## Tokens

- Canvas: `#0e0f12`; surfaces: `#15171b`, `#1a1d22`, and `#101216`.
- Text: `#e6e9ec`; secondary text: `#9aa3ac`; faint text: `#69727b`.
- Amber accent: `#d2a24c`; brighter active accent: `#e3b75f`.
- Status colors are reserved for real state only: green for success and red for errors.
- `Inter`/system sans is for prose and controls. `IBM Plex Mono`/system monospace is for speaker labels, timestamps, statuses, counts, and identifiers. No remote fonts; no font files are currently bundled.
- Corners are 3–4px. Use surface changes and borders for grouping, with no shadows, glow, or animated decoration.

## Transcript

- Keep all archived text, Markdown, code blocks, links, and source graph data intact.
- For real graph messages, map the existing `role` field to `data-speaker` and an honest visible label: `user` → “YOU”, `assistant` → “ASSISTANT”; preserve system/developer/tool labels as present.
- Imported single-message archive transcripts remain one saved transcript. If their rendered Markdown contains direct, literal `You` or `ChatGPT` level-two headings, the renderer may group each heading and following blocks into presentational `.archive-turn` sections. These wrappers never create message IDs or branch claims. Ignore matching words inside quotes, code, and formatted headings.
- User turns use a restrained warm surface and amber rule. Assistant turns use a neutral graphite surface and ivory rule. Keep role labels in monospace; keep the message body in the prose font with comfortable line height.
- When source text does not identify a speaker, show a generic “Message” label and do not infer a role.

## Workspace

- Retain the compact library sidebar, tab strip, and reading/live-chat canvas. The map and overview remain content tools, not a second navigation system.
- Use amber for the active source/action and role markers, not as a general fill color. Entity kinds can use low-contrast amber/stone variations; shape and labels remain the primary distinctions.
- Archive text must stay comfortable to read: body size around 14px, line height about 1.65, constrained reading width, and readable links, tables, lists, blockquotes, and fenced code.
- Keep visible keyboard focus, accessible labels, and reduced-motion behavior. No browser tests or driver launches for visual changes; validate pure renderer behavior with Bun tests and native builds.
