nonce => {
  const url = new URL(location.href);
  const exactOrigin = url.protocol === 'https:' &&
    url.hostname === 'chatgpt.com' && url.port === '' &&
    url.username === '' && url.password === '';
  let conversationId = null;
  if (exactOrigin && url.search === '' && url.hash === '') {
    const match = /^\/c\/([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})$/.exec(url.pathname);
    if (match) conversationId = match[1];
  }
  return JSON.stringify({nonce, exact_origin: exactOrigin, conversation_id: conversationId});
}
