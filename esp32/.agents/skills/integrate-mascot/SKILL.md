---
name: integrate-mascot
description: Integrate completed Masko images, animations or interactive releases into the user's website or app. Use for HTML, React, Swift/SwiftUI and platform-specific playback guidance.
---

# Integrate a mascot

These instructions require Masko's hosted MCP at https://masko.ai/api/mcp.
For Cursor, follow the [connection guide](https://masko.ai/docs/ai-tools/cursor),
sign in through OAuth, and choose the user's personal or team workspace.
Skills-only installation adds instructions; configure and authorize the MCP
separately in the host. Other hosts need their own supported Masko connection.
A safe first check is to list projects, mascots and credits without generating.

Use Masko's connected MCP tools for this workflow. Discover the MCP tools
before declaring them unavailable. If they are unavailable, or the host blocks
an action for approval, explain the exact limitation and stop the affected step.
Do not substitute the Masko Mac app, another account, or a direct API credential
to make an MCP operation appear successful. Do not change host permissions.

Use the platform already established by the user or repository. Otherwise ask
whether the target is a website, React app, macOS app, iOS app or another target.
Use `get_integration_guide` with `topic: "integrations/playback"` before writing
playback code. Implement the player directly in the user's platform. Do not
install or recommend `@masko/sdk` for this integration.

Retrieve existing results with `get_asset`, `list_assets`, `get_mascot_cdn`, or
`get_release_delivery`; do not regenerate a character merely to integrate it.
Use `export_asset` only for missing required formats and wait for its result.

Choose the smallest player that handles the requested behavior:

- One unchanged looping animation: a simple native/video player.
- Changing clips: two persistent players with verified-frame handoffs.
- Interactive graph: add a controller that respects declared inputs, conditions,
  priorities and return paths. Two buffers alone are not a state machine. Read
  `canvas/build` and `canvas/export` for the graph contract; do not invent methods.

For every clip change, preserve the outgoing frame while the inactive player
loads and decodes the next one. Commit only after both a verified candidate frame
and the permitted graph boundary. Swap once without crossfading, pause the old
slot, and release it only after replacement presentation. Never clear/remount the
visible player while preparing, or
use a timeout as permission to reveal an unready player. On failure, retain the
old frame or initial poster. Guard ready/end/error/cleanup callbacks with request
and slot-version identities so stale work cannot swap or clear a reused player.

On web, use current-source video frame callbacks and a persistent canvas or
atomic video-layer swap. On Apple platforms, use persistent AVPlayer layers,
layer readiness plus a decoded pixel buffer, and one CATransaction with implicit
animations disabled. On Android, require a rendered frame before switching the
selected surface/texture. The guide covers each handoff and its failure behavior.

Select actual supported media formats; verify transparent alpha instead of
assuming any MP4 is transparent. Handle reduced motion, autoplay blocking,
background/resume and disposal. Drive completion from media events rather than
fixed duration timers. Verify idle → action → idle, rapid requests, slow/failing
loads, and transparency over light/dark backgrounds. Do not claim visual parity
with Desktop or Life from code generation alone.

Production private-release delivery belongs behind the user's authorized backend.
Forward only the signed delivery payload to their frontend. Never put Masko
authoring credentials or OAuth tokens in client code. Signed URLs expire; show
how to obtain fresh authorized delivery rather than hard-coding temporary links.
For permanent public CDN URLs, explain what becomes public and use
`publish_mascot_assets` only when the user requests or approves public sharing.

Write the integration in the user's existing project style. Map requested app
events to documented behavior/actions, then run relevant build and playback
checks when the environment permits. State which platform and behaviors were
actually tested. Do not claim to have installed, deployed or visually verified
an integration based only on generated code.
