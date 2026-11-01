export function appendPinMarker(titleElement, pinned, doc=titleElement?.ownerDocument||document) {
  if (!pinned || !titleElement) return null;
  const marker=doc.createElement('span');
  marker.className='conversation-pin-marker';
  marker.setAttribute('role','img');
  marker.setAttribute('aria-label','Pinned in ChatGPT');
  const svg=doc.createElementNS('http://www.w3.org/2000/svg','svg');
  svg.setAttribute('viewBox','0 0 24 24');svg.setAttribute('aria-hidden','true');
  const path=doc.createElementNS('http://www.w3.org/2000/svg','path');
  path.setAttribute('d','M8 3h8v6l3 4v3H5v-3l3-4V3Zm4 13v5');svg.append(path);marker.append(svg);
  titleElement.append(marker);
  return marker;
}
