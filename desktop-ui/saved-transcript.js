import {messageText,renderMarkdown,speakerPresentation,decorateArchiveTranscript} from './render.js';

export function speakerLabel(message) {
  const author=message?.author;
  const role=typeof message?.role==='string'?message.role.trim()
    :typeof author==='string'?author.trim()
    :author&&typeof author==='object'&&typeof author.role==='string'?author.role.trim():'';
  const presentation=speakerPresentation({role});
  if(presentation.key!=='unknown'||presentation.label!=='Message')return presentation;
  const name=author&&typeof author==='object'
    ? [author.name,author.display_name,author.displayName].find(value=>typeof value==='string'&&value.trim())
    : null;
  return name?{key:'unknown',label:name.trim()}:presentation;
}

function appendTextChunk(message,body,text,{doc,disableLinks}) {
  if(text){
    const chunk=doc.createElement('div');chunk.className='saved-turn__chunk';body.append(chunk);renderMarkdown(text,chunk,doc);
    if(speakerLabel(message).key==='archive')decorateArchiveTranscript(chunk,doc);
  }
  if(disableLinks&&!body.dataset.linksDisabled){
    body.dataset.linksDisabled='true';
    body.addEventListener('click',event=>{if(event.target.closest?.('a'))event.preventDefault();});
  }
}

function createTurn(message,root,{doc,continuation=false}={}) {
  const speaker=speakerLabel(message);
  const turn=doc.createElement('article');turn.className='saved-turn';turn.dataset.speaker=speaker.key;
  if(continuation)turn.classList.add('saved-turn--continuation');
  else {const heading=doc.createElement('header');heading.className='saved-turn__speaker';heading.textContent=speaker.label;turn.append(heading);}
  const body=doc.createElement('div');body.className='saved-turn__content';turn.append(body);root.append(turn);
  return body;
}

export function renderSavedTurn(message,root,{doc=document,disableLinks=false}={}) {
  const body=createTurn(message,root,{doc});
  appendTextChunk(message,body,messageText(message),{doc,disableLinks});
  return body.parentNode;
}

const fenceLine=line=>line.match(/^\s*(`{3,}|~{3,})(.*)$/);
function updateFenceState(text,state) {
  const lines=text.split('\n');
  for(const line of lines){
    const match=fenceLine(line);if(!match)continue;
    const marker=match[1],char=marker[0];
    if(!state)state={char,length:marker.length,opening:line};
    else if(char===state.char&&marker.length>=state.length)state=null;
  }
  return state;
}
function nextChunk(text,offset,maxChars,fenceState) {
  if(offset>=text.length)return {end:text.length,rendered:'',fenceState};
  let end=Math.min(text.length,offset+maxChars);
  if(end<text.length){
    const floor=offset+Math.floor(maxChars*.55);
    const blank=text.lastIndexOf('\n\n',end);
    const newline=text.lastIndexOf('\n',end);
    if(blank>=floor)end=blank+2;
    else if(newline>=floor)end=newline+1;
    else {const space=text.lastIndexOf(' ',end);if(space>=floor)end=space+1;}
  }
  let raw=text.slice(offset,end),nextFence=updateFenceState(raw,fenceState);
  let rendered=raw;
  if(fenceState)rendered=`${fenceState.opening}\n${rendered}`;
  if(nextFence)rendered+=`\n${nextFence.char.repeat(nextFence.length)}\n`;
  return {end,rendered,fenceState:nextFence};
}

export function createTranscriptPager(container,messages,{batchSize=8,maxChunkChars=24_000,doc=document,disableLinks=false,emptyText='No message text is available in this saved transcript.'}={}) {
  const list=Array.isArray(messages)?messages:[];let messageIndex=0,offset=0,currentMessage=null,currentText='',currentBody=null,fenceState=null,destroyed=false,adding=false,chunkCount=0;
  const more=doc.createElement('button');more.type='button';more.className='saved-transcript-more';more.textContent='Load more saved content';more.setAttribute('aria-label','Load more saved conversation content');
  const removeMore=()=>{if(more.parentNode)more.parentNode.removeChild(more);};
  const loadNext=()=>{
    if(destroyed||adding||messageIndex>=list.length)return;
    adding=true;removeMore();let added=0;
    while(messageIndex<list.length&&added<Math.max(1,batchSize)){
      if(!currentMessage){
        currentMessage=list[messageIndex];currentText=messageText(currentMessage);offset=0;fenceState=null;
        currentBody=createTurn(currentMessage,container,{doc});
      }
      if(!currentText.length){
        messageIndex++;currentMessage=null;currentBody=null;added++;chunkCount++;continue;
      }
      const chunk=nextChunk(currentText,offset,maxChunkChars,fenceState);
      appendTextChunk(currentMessage,currentBody,chunk.rendered,{doc,disableLinks});
      offset=chunk.end;fenceState=chunk.fenceState;added++;chunkCount++;
      if(offset>=currentText.length){messageIndex++;currentMessage=null;currentText='';currentBody=null;offset=0;fenceState=null;}
    }
    if(messageIndex<list.length)container.append(more);
    adding=false;
  };
  if(!list.length){const empty=doc.createElement('p');empty.className='saved-transcript-empty';empty.textContent=emptyText;container.append(empty);}
  else {more.addEventListener('click',loadNext);loadNext();}
  const onScroll=()=>{if(messageIndex<list.length&&container.scrollHeight-container.scrollTop-container.clientHeight<180)loadNext();};
  container.addEventListener('scroll',onScroll,{passive:true});
  return {
    get loaded(){return chunkCount;},
    get total(){return list.length;},
    loadNext,
    destroy(){destroyed=true;container.removeEventListener('scroll',onScroll);removeMore();},
  };
}
