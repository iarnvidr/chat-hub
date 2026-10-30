# ChatGPT JSON import

Run `chat-hub import <path>` with the same configuration as the existing CLI.
`CHAT_HUB_DATA_DIR` and `CHAT_HUB_DATABASE` resolve identically to other commands.
The input is a file path only; stdin is not a special input mode.

The shared importer accepts the UTF-8 `conversations.json` array, or an object
containing a `conversations` array. Extract that file before importing a ZIP.
Inputs are limited to 67,108,864 actual bytes and 10,000 conversation entries.
An empty array is valid. The complete global envelope is validated before any
conversation is committed. Invalid individual entries fail independently.

Synthetic export examples live in `tests/fixtures/chatgpt-export`. The array
example uses the actual export's top-level `id` identity variant; the object
example uses `conversation_id`. Neither fixture contains real account data.
Export shape must not be inferred solely from the live detail response shape.

Each successful conversation commits independently. A timeout, interrupted CLI,
or HTTP disconnect may leave earlier conversations committed; it does not imply
batch rollback. Retry the same file to resolve uncertainty through idempotence.
Receipts count durable outcomes and retain only the first 100 safe item errors.
The initial identity scope is `chatgpt` / `provisional:default`, with no guessed
account identity. Import admission and HTTP transport remain adapter concerns.
