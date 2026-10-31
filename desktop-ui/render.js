export function selectedPath(conversation, selectedId) {
 const nodes=new Map(conversation.nodes.map(n=>[n.id,n])); let id=selectedId;
 if(!nodes.has(id)) throw Error('Selected branch is missing. Choose a branch explicitly.');
 const path=[],seen=new Set(); while(id!==null){if(seen.has(id))throw Error('Conversation graph contains a cycle.');seen.add(id);const n=nodes.get(id);if(!n)throw Error('Conversation graph has a missing parent.');path.push(n);id=n.parent_id;} return path.reverse().filter(n=>n.message_id);
}
export function branchChoices(c){const parents=new Set(c.nodes.map(n=>n.parent_id).filter(x=>x!==null));return (c.branches||[]).map(b=>typeof b==='string'?b:b.leaf_node_id).map(id=>c.nodes.find(n=>n.id===id)).filter(Boolean);}
export function orderedMessages(detail,leafNodeId=detail.selection?.selected_node_id){const nodes=new Map(detail.nodes.map(n=>[n.id,n])),messages=new Map(detail.messages.map(m=>[m.id,m]));const ids=leafNodeId===detail.selection?.selected_node_id?detail.thread_node_ids:selectedPath(detail,leafNodeId).map(n=>n.id);const seen=new Set();return ids.map(id=>{if(seen.has(id))throw Error('Conversation graph contains a cycle.');seen.add(id);const n=nodes.get(id);if(!n)throw Error('Conversation graph has a missing node.');if(n.message_id==null)return null;const m=messages.get(n.message_id);if(!m)throw Error('Conversation graph has a missing message.');return m;}).filter(Boolean);}
export function messageText(m){if(typeof m.content==='string'&&m.content.length)return m.content;const parts=Array.isArray(m.parts)?m.parts:[];return parts.filter(p=>p?.kind==='text'&&typeof p.text==='string').map(p=>p.text).join('');}
export function hasUnsupportedContent(m){return Array.isArray(m.parts)&&m.parts.some(p=>p?.kind==='unknown');}
export function renderMarkdown(markdownSource, root, doc=document) {
  const lines = String(markdownSource ?? '').replace(/\r\n?/g, '\n').split('\n');

  function text(parent, value) {
    if (value) parent.append(doc.createTextNode(value));
  }

  function findClosing(source, start, delimiter) {
    for (let at = start + delimiter.length; at < source.length;) {
      if (source[at] === '\\') { at += 2; continue; }
      if (source[at] === '`') {
        let count = 1;
        while (source[at + count] === '`') count++;
        const fence = '`'.repeat(count), close = source.indexOf(fence, at + count);
        if (close >= 0) { at = close + count; continue; }
      }
      if (source.startsWith(delimiter, at)) {
        let run = 0;
        while (source[at + run] === delimiter[0]) run++;
        const previous = source[at - 1], next = source[at + run];
        if (previous && !/\s/.test(previous) && (!next || /\s|[.,!?;:)\]}]/.test(next))) {
          return run > delimiter.length ? at + run - delimiter.length : at;
        }
        at += run;
      } else at++;
    }
    return -1;
  }

  function linkAt(source, start) {
    let depth = 0, closeLabel = -1;
    for (let i = start; i < source.length; i++) {
      if (source[i] === '\\') { i++; continue; }
      if (source[i] === '[') depth++;
      else if (source[i] === ']' && --depth === 0) { closeLabel = i; break; }
    }
    if (closeLabel < 0 || source[closeLabel + 1] !== '(') return null;
    let parens = 1;
    for (let i = closeLabel + 2; i < source.length; i++) {
      if (source[i] === '\\') { i++; continue; }
      if (source[i] === '(') parens++;
      else if (source[i] === ')' && --parens === 0) {
        return {label: source.slice(start + 1, closeLabel), destination: source.slice(closeLabel + 2, i).trim(), end: i + 1};
      }
    }
    return null;
  }

  function inline(parent, source) {
    let i = 0, plain = '';
    const flush = () => { text(parent, plain); plain = ''; };
    while (i < source.length) {
      if (source[i] === '\\' && i + 1 < source.length && /[\\`*_{}\[\]()#+.!|>~-]/.test(source[i + 1])) {
        plain += source[i + 1]; i += 2; continue;
      }
      if (source[i] === '`') {
        let count = 1;
        while (source[i + count] === '`') count++;
        const delimiter = '`'.repeat(count), end = source.indexOf(delimiter, i + count);
        if (end >= 0) {
          flush();
          const code = doc.createElement('code');
          let value = source.slice(i + count, end).replace(/\n/g, ' ');
          if (/^ .* $/.test(value) && /\S/.test(value)) value = value.slice(1, -1);
          code.textContent = value; parent.append(code); i = end + count; continue;
        }
      }
      if (source[i] === '[') {
        const link = linkAt(source, i);
        if (link) {
          let url;
          try { url = new URL(link.destination); } catch { url = null; }
          if (url?.protocol === 'https:') {
            flush();
            const anchor = doc.createElement('a');
            anchor.href = url.href; anchor.target = '_blank'; anchor.rel = 'noopener noreferrer';
            inline(anchor, link.label); parent.append(anchor); i = link.end; continue;
          }
        }
      }

      const delimiter = source.startsWith('***', i) ? '***'
        : source.startsWith('**', i) ? '**'
        : source.startsWith('__', i) ? '__'
        : source.startsWith('~~', i) ? '~~'
        : source[i] === '*' || source[i] === '_' ? source[i] : null;
      const previous = source[i - 1];
      const underscoreInWord = delimiter?.[0] === '_' && previous && /[A-Za-z0-9_]/.test(previous);
      if (delimiter && !underscoreInWord && source[i + delimiter.length] && !/\s/.test(source[i + delimiter.length])) {
        const end = findClosing(source, i, delimiter);
        if (end > i + delimiter.length) {
          flush();
          if (delimiter === '***') {
            const strong = doc.createElement('strong'), emphasis = doc.createElement('em');
            inline(emphasis, source.slice(i + 3, end)); strong.append(emphasis); parent.append(strong);
          } else {
            const tag = delimiter === '**' || delimiter === '__' ? 'strong' : delimiter === '~~' ? 'del' : 'em';
            const element = doc.createElement(tag); inline(element, source.slice(i + delimiter.length, end)); parent.append(element);
          }
          i = end + delimiter.length; continue;
        }
      }
      plain += source[i++];
    }
    flush();
  }

  function splitTableRow(line) {
    let value = line.trim();
    if (value.startsWith('|')) value = value.slice(1);
    if (value.endsWith('|') && !value.endsWith('\\|')) value = value.slice(0, -1);
    const cells = []; let cell = '', ticks = 0;
    for (let i = 0; i < value.length; i++) {
      const char = value[i];
      if (char === '\\' && i + 1 < value.length) { cell += char + value[++i]; continue; }
      if (char === '`') {
        let count = 1;
        while (value[i + count] === '`') count++;
        if (!ticks) ticks = count;
        else if (ticks === count) ticks = 0;
        cell += '`'.repeat(count); i += count - 1; continue;
      }
      if (char === '|' && !ticks) { cells.push(cell.trim()); cell = ''; }
      else cell += char;
    }
    cells.push(cell.trim());
    return cells;
  }

  function tableAt(index) {
    if (index + 1 >= lines.length || !lines[index].includes('|')) return null;
    const header = splitTableRow(lines[index]), delimiters = splitTableRow(lines[index + 1]);
    if (!header.length || delimiters.length !== header.length ||
        !delimiters.every(value => /^:?-+:?$/.test(value))) return null;
    const alignments = delimiters.map(value => value.startsWith(':') && value.endsWith(':') ? 'center'
      : value.endsWith(':') ? 'right' : value.startsWith(':') ? 'left' : '');
    let end = index + 2;
    while (end < lines.length && lines[end].trim() && lines[end].includes('|')) end++;
    return {header, alignments, rows: lines.slice(index + 2, end).map(splitTableRow), end};
  }

  const listMarker = line => /^( *)([-+*]|\d+[.)])\s+(.*)$/.exec(line);
  const isRule = line => /^(?:\s{0,3})(?:(?:\*\s*){3,}|(?:-\s*){3,}|(?:_\s*){3,})$/.test(line);
  const isFence = line => /^ {0,3}`{3,}/.test(line);
  const isQuote = line => /^ {0,3}>/.test(line);
  const isHeading = line => /^ {0,3}#{1,6}(?:\s|$)/.test(line);
  const startsBlock = (source, index) => isFence(source[index]) || isHeading(source[index]) || isRule(source[index]) ||
    isQuote(source[index]) || !!listMarker(source[index]) || !!tableAt(index);

  function renderList(sourceLines, start, parent, first) {
    const indent = first[1].length, ordered = /^\d/.test(first[2]);
    const list = doc.createElement(ordered ? 'ol' : 'ul');
    if (ordered && first[2].match(/^\d+/)[0] !== '1') list.start = Number(first[2].match(/^\d+/)[0]);
    parent.append(list);
    let i = start;
    while (i < sourceLines.length) {
      const marker = listMarker(sourceLines[i]);
      if (!marker || marker[1].length !== indent || /^\d/.test(marker[2]) !== ordered) break;
      const item = doc.createElement('li'); inline(item, marker[3]); list.append(item); i++;
      const continuation = [];
      while (i < sourceLines.length) {
        if (!sourceLines[i].trim()) {
          if (i + 1 < sourceLines.length && (sourceLines[i + 1].startsWith(' '.repeat(indent + 2)) ||
              (listMarker(sourceLines[i + 1]) && listMarker(sourceLines[i + 1])[1].length === indent))) {
            continuation.push(''); i++; continue;
          }
          break;
        }
        const next = listMarker(sourceLines[i]);
        if (next && next[1].length === indent) break;
        const spaces = sourceLines[i].match(/^ */)[0].length;
        if (spaces < indent + 2) break;
        continuation.push(sourceLines[i].slice(Math.min(sourceLines[i].length, indent + 2))); i++;
      }
      if (continuation.some(line => line.trim())) renderBlocks(continuation, item);
      if (i < sourceLines.length && !sourceLines[i].trim()) break;
    }
    return i;
  }

  function renderBlocks(sourceLines, parent) {
    let i = 0;
    while (i < sourceLines.length) {
      const line = sourceLines[i];
      if (!line.trim()) { i++; continue; }

      if (isFence(line)) {
        const opener = /^ {0,3}(`{3,})(.*)$/.exec(line), fence = opener[1], language = opener[2].trim();
        const codeLines = []; i++;
        while (i < sourceLines.length) {
          const closer = /^ {0,3}(`+)[ \t]*$/.exec(sourceLines[i]);
          if (closer && closer[1].length >= fence.length) break;
          codeLines.push(sourceLines[i++]);
        }
        if (i < sourceLines.length) i++;
        const pre = doc.createElement('pre'), code = doc.createElement('code');
        if (language) code.dataset.language = language;
        code.textContent = codeLines.join('\n'); pre.append(code); parent.append(pre); continue;
      }

      if (isHeading(line)) {
        const heading = /^ {0,3}(#{1,6})(?:\s+(.*)|\s*)$/.exec(line), element = doc.createElement(`h${heading[1].length}`);
        inline(element, heading[2] || ''); parent.append(element); i++; continue;
      }
      if (isRule(line)) { parent.append(doc.createElement('hr')); i++; continue; }

      if (isQuote(line)) {
        const quoted = [];
        while (i < sourceLines.length) {
          if (isQuote(sourceLines[i])) quoted.push(sourceLines[i++].replace(/^ {0,3}> ?/, ''));
          else if (!sourceLines[i].trim() && i + 1 < sourceLines.length && isQuote(sourceLines[i + 1])) { quoted.push(''); i++; }
          else break;
        }
        const quote = doc.createElement('blockquote'); renderBlocks(quoted, quote); parent.append(quote); continue;
      }

      const parsedTable = tableAt(i);
      if (parsedTable) {
        const table = doc.createElement('table'); table.className = 'markdown-table';
        const head = doc.createElement('thead'), headRow = doc.createElement('tr');
        parsedTable.header.forEach((value, column) => {
          const cell = doc.createElement('th'); cell.scope = 'col';
          if (parsedTable.alignments[column]) cell.style.textAlign = parsedTable.alignments[column];
          inline(cell, value); headRow.append(cell);
        });
        head.append(headRow); table.append(head);
        const body = doc.createElement('tbody');
        for (const row of parsedTable.rows) {
          const tr = doc.createElement('tr');
          for (let column = 0; column < parsedTable.header.length; column++) {
            const cell = doc.createElement('td');
            if (parsedTable.alignments[column]) cell.style.textAlign = parsedTable.alignments[column];
            inline(cell, row[column] || ''); tr.append(cell);
          }
          body.append(tr);
        }
        table.append(body); parent.append(table); i = parsedTable.end; continue;
      }

      const marker = listMarker(line);
      if (marker) { i = renderList(sourceLines, i, parent, marker); continue; }

      const paragraph = [line]; i++;
      while (i < sourceLines.length && sourceLines[i].trim() && !startsBlock(sourceLines, i)) paragraph.push(sourceLines[i++]);
      const p = doc.createElement('p'); inline(p, paragraph.join(' ')); parent.append(p);
    }
  }

  root.replaceChildren();
  renderBlocks(lines, root);
}
