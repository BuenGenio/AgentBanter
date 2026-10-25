// Cold outside, warm inside. Depth is also numbered, so colour is never the only cue.
(() => {
  const colours=['#b6a1f2','#91acf7','#72c2ee','#71cdbf','#a3cf89','#e4d67d','#edaf77','#ef8d88'];
  const boundary='details, .card, .tools';
  function depth(node){
    let level=0;
    for(let parent=node.parentElement?.closest(boundary);parent;parent=parent.parentElement?.closest(boundary))level++;
    return level;
  }
  function paint(root){
    const nodes=[];
    if(root.matches?.('details, .card, .tools, .tree-leaf'))nodes.push(root);
    nodes.push(...root.querySelectorAll('details, .card, .tools, .tree-leaf'));
    for(const node of nodes){
      const level=depth(node);
      node.dataset.nestLevel=String(level+1);
      node.style.setProperty('--nest-colour',colours[Math.min(level,colours.length-1)]);
      node.classList.add('depth-surface');
      if(node.tagName!=='DETAILS')continue;
      const summary=node.querySelector(':scope > summary');if(!summary)continue;
      if(!node.classList.contains('structured-disclosure')){
        node.classList.add('structured-disclosure');
        const hint=document.createElement('span');hint.className='disclosure-hint';hint.dataset.searchIgnore='true';
        summary.append(hint);
        node.addEventListener('toggle',()=>{hint.textContent='L'+node.dataset.nestLevel+' · '+(node.open?'Collapse':'Expand');});
      }
      summary.querySelector(':scope > .disclosure-hint').textContent='L'+(level+1)+' · '+(node.open?'Collapse':'Expand');
    }
  }
  paint(document);
  const pending=new Set();let scheduled=false;
  const observer=new MutationObserver(records=>{
    for(const record of records)for(const node of record.addedNodes){
      if(node.nodeType!==1||node.matches('.disclosure-hint'))continue;
      pending.add(node);
    }
    if(pending.size&&!scheduled){scheduled=true;requestAnimationFrame(()=>{
      scheduled=false;for(const node of pending)if(node.isConnected)paint(node);pending.clear();
    });}
  });
  observer.observe(document.body,{childList:true,subtree:true});
})();
