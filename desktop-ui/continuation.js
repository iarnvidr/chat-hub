// Paseo uses its own document identity; only ChatGPT conversation UUIDs can
// address a live chat. Never treat a cached document's arbitrary URL as a target.
export function chatgptConversationId(conversation) {
  if (!['chatgpt', 'paseo-chatgpt'].includes(conversation?.provider)) return null;
  let id = conversation.source_id;
  if (typeof id !== 'string') return null;
  if (conversation.provider === 'paseo-chatgpt' && id.startsWith('conversation:')) {
    id = id.slice('conversation:'.length);
  }
  return /^[0-9a-f]{8}-(?:[0-9a-f]{4}-){3}[0-9a-f]{12}$/i.test(id) ? id : null;
}
