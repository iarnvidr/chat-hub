# Saved copies and conversation identity

Chat Hub can have more than one saved copy of a ChatGPT conversation: an older
Paseo import and a directly observed ChatGPT capture. These are separate source
records, but they can describe the same upstream conversation and project.

The library resolves those identities when reading. It does not rewrite saved
transcripts, change account ownership, or merge projects by their display names.
An ambiguous match remains separate. The shared identity projection is used by
project filters, chat lists, search, and the catalog graph.

Automatic matching currently covers `paseo-chatgpt` records in `paseo-cache:*`
scopes and direct `chatgpt` records in verified-format `chatgpt:observed:*`
scopes. A conversation needs an exact valid upstream UUID (allowing the old
single `conversation:` prefix) and one unambiguous observed match. Project
matching additionally needs the same upstream project ID and a shared resolved
chat. Other providers and import scopes remain separate; equal names alone are
never identity evidence. An observed scope's format does not grant permission
to change an upstream account.

The directly observed copy supplies the current title, project membership, and
pin/archive state. An imported conversation without a directly observed copy
remains available. Local tags are read across matching saved copies; replacing
those tags must update the whole resolved group so removed tags cannot reappear
from an older copy.

Search still examines every saved copy. A result's `conversation` is the logical
display and action target, while `matched_conversation_id` identifies the saved
copy containing its message, node, and branch anchors. Clients must open that
matching copy when navigating to a search hit. Catalog references likewise keep
their physical source and message coordinates even when the map displays one
logical chat.

Search pagination orders and encodes its cursor using the matched physical
conversation ID, not the displayed logical ID. Several saved copies can produce
hits for one displayed chat; grouping those hits must not change the API cursor.

This is a read projection over the existing schema, not a destructive database
cleanup. Importing the same history again must not create extra logical rows or
discard older-only messages, tags, or indexed references.
