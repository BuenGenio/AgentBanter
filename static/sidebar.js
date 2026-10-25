(() => {
  const list=document.querySelector('.conversation-list');if(!list)return;
  const branches=[...list.querySelectorAll('.conversation-branch')],groups=[...list.querySelectorAll('.conversation-group')];
  let globalMatches=null;
  window.filterConversationDrawer=ids=>{globalMatches=ids===null?null:new Set(ids);apply();};
  function apply(){
    const filtering=globalMatches!==null;
    for(const branch of [...branches].reverse()){
      const own=globalMatches===null||globalMatches.has(branch.dataset.sessionId);
      const child=[...branch.querySelectorAll(':scope > .conversation-children > .conversation-branch')].some(c=>!c.hidden);
      branch.hidden=!(own||child);branch.classList.toggle('ancestor-context',!own&&child);
      if(filtering&&child)branch.open=true;
    }
    for(const group of groups){group.hidden=![...group.querySelectorAll('.conversation-branch')].some(b=>!b.hidden);if(filtering&&!group.hidden)group.open=true;}
    document.getElementById('search-empty').hidden=groups.some(g=>!g.hidden);
  }
  for(const branch of list.querySelectorAll('.hover-branch')){
    let timer,opening,temporary=false;
    branch.addEventListener('pointerenter',()=>{clearTimeout(timer);clearTimeout(opening);opening=setTimeout(()=>{if(!branch.open&&branch.matches(':hover')){temporary=true;branch.open=true;}},150);});
    branch.addEventListener('pointerleave',()=>{clearTimeout(opening);clearTimeout(timer);timer=setTimeout(()=>{
      if(temporary&&!branch.matches(':hover,:focus-within')){branch.open=false;temporary=false;}
    },180);});
    branch.addEventListener('focusin',()=>{branch.open=true;});
  }
  const selected=list.querySelector('a.selected');
  for(let parent=selected?.closest('details');parent;parent=parent.parentElement?.closest('details'))parent.open=true;
  try{list.scrollTop=Number(sessionStorage.getItem('agentbanter.sidebar-scroll')||0);list.addEventListener('scroll',()=>sessionStorage.setItem('agentbanter.sidebar-scroll',list.scrollTop),{passive:true});}catch{}
})();
