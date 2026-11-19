export function describeBuildIdentity(value) {
  const valid = id => typeof id === 'string' && /^[a-f0-9]{40}$/.test(id);
  if (value?.schema !== 1 || !valid(value.source_commit) || !valid(value.source_tree)) {
    return 'Source build identity unavailable';
  }
  return `Native source ${value.source_commit.slice(0, 12)} · tree ${value.source_tree.slice(0, 12)}`;
}

if (typeof document !== 'undefined') {
  const version = document.getElementById('version');
  if (version) {
    const label = document.createElement('p');
    label.id = 'build-identity';
    label.textContent = 'Loading source build identity…';
    label.title = 'Identifies this native binary. The stable updater channel is separate from the GitHub preview release channel.';
    version.insertAdjacentElement('afterend', label);
    fetch('./build-identity.json', {cache: 'no-store'})
      .then(response => response.ok ? response.json() : null)
      .then(value => { label.textContent = describeBuildIdentity(value); })
      .catch(() => { label.textContent = describeBuildIdentity(null); });
  }
}
