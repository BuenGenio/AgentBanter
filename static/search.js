// Search normalized transcript blocks, including content hidden inside inspectors.
(() => {
  const $ = id => document.getElementById(id);
  const make = (tag, text, cls) => {
    const node = document.createElement(tag);
    if (text !== undefined) node.textContent = text;
    if (cls) node.className = cls;
    return node;
  };
  const sid = $('task-search')?.dataset.session;
  const kinds = {prompt:'Your message',reply:'Agent reply',command:'Command',tool:'Tool run',inferred:'Inferred decision',decision:'Decision',reasoning:'Reasoning summary'};
  let current = -1, matches = [], active = false;
  function reveal(id) {
    const target = $(id);
    if (!target?.closest('#timeline')) return;
    if ($('task-results')) $('task-results').open=false;
    const follow = $('t-follow'); if (follow) follow.checked = false;
    for (let node = target; node; node = node.parentElement) if (node.tagName === 'DETAILS') node.open = true;
    const detail = target.querySelector(':scope > details'); if (detail) detail.open = true;
    target.closest('.step')?.classList.add('search-reveal');
    target.classList.add('search-reveal');
    // A hit may be in a decoded JSON string rather than the default Output tab.
    const query = $('task-query')?.value || '';
    if (query && !$('task-regex')?.checked) {
      for (const host of target.querySelectorAll('.output-inspector')) window.revealInspectorMatch?.(host, query, $('task-case')?.checked);
    }
    document.querySelectorAll('.search-current').forEach(n => n.classList.remove('search-current'));
    target.classList.add('search-current');
    target.tabIndex = -1; target.focus({preventScroll:true});
    target.scrollIntoView({block:'center',behavior:matchMedia('(prefers-reduced-motion: reduce)').matches?'instant':'smooth'});
  }
  function hitLink(hit, globalResult = false) {
    const link = make('a');
    link.href = '/s/' + encodeURIComponent(hit.session) + '#' + encodeURIComponent(hit.anchor);
    link.append(make('span', (kinds[hit.kind] || hit.kind) + ' · ' + hit.agent, 'search-hit-kind'), make('span', hit.excerpt, 'search-excerpt'));
    if (hit.session === sid) link.addEventListener('click', e => {
      if (e.ctrlKey || e.metaKey || e.shiftKey || e.altKey) return;
      e.preventDefault(); if(globalResult)window.clearConversationSearch?.(); $('global-results').hidden=true; history.replaceState(null, '', link.hash);
      current = matches.indexOf(hit.anchor); reveal(hit.anchor);
    });
    return link;
  }
  function renderHits(host, data) {
    host.replaceChildren();
    const notices=make('details',undefined,'search-notices');notices.append(make('summary','Indexing limits · '+data.warnings.length+' transcripts'));
    for(const warning of data.warnings)notices.append(make('p',warning));
    if(data.warnings.length){const item=make('li');item.append(notices);host.append(item);}
    const groups = new Map();
    for (const hit of data.hits) {
      let list = groups.get(hit.session);
      if (!list) {
        const li = make('li'), details = make('details', undefined, 'search-result-group');
        details.open = true;
        details.append(make('summary', hit.title + ' · ' + hit.agent));
        list = make('ol'); details.append(list); li.append(details); host.append(li); groups.set(hit.session, list);
      }
      const li = make('li'); li.append(hitLink(hit,host.id==='global-hits')); list.append(li);
    }
  }
  function description(data) {
    return data.count + ' matching block' + (data.count === 1 ? '' : 's') + ' in ' + data.sessions.length + ' conversation' + (data.sessions.length === 1 ? '' : 's') +
      (data.limited ? ' · Showing the first 100 results' : '') + (data.warnings.length ? ' · '+data.warnings.length+' indexing notices' : '');
  }
  function requester(run) {
    let controller, serial = 0, timer;
    async function request(params, accept, fail) {
      const mine = ++serial;
      controller?.abort(); controller = new AbortController();
      try {
        const response = await fetch('/search?' + new URLSearchParams(params), {signal:controller.signal});
        const data = await response.json();
        if (mine !== serial) return;
        if (!response.ok) throw new Error(data.error || 'Search failed.');
        accept(data);
      } catch (error) { if (error.name !== 'AbortError' && mine === serial) fail(error.message); }
    }
    return {
      request,
      cancel() { clearTimeout(timer); ++serial; controller?.abort(); },
      schedule() { clearTimeout(timer); timer = setTimeout(run, 300); }
    };
  }

  const global = $('global-query'), globalRequest = requester(globalSearch);
  function globalSearch(show = true) {
    globalRequest.cancel();
    try {sessionStorage.setItem('agentbanter.global-search', JSON.stringify({q:global.value,mode:$('global-mode').value,regex:$('global-regex').checked}));} catch {}
    if (!global.value.trim()) {
      $('global-results').hidden = true; window.filterConversationDrawer?.(null); return;
    }
    $('global-results').hidden = show === false; $('global-status').textContent = 'Searching all recorded conversations…';
    globalRequest.request({q:global.value,regex:$('global-regex').checked}, data => {
      renderHits($('global-hits'), data); $('global-status').textContent = description(data);
      window.filterConversationDrawer?.($('global-mode').value === 'filter' ? data.sessions : null);
    }, error => { $('global-status').textContent = error; $('global-hits').replaceChildren(); window.filterConversationDrawer?.(null); });
  }
  global.addEventListener('focus',()=>{if(global.value.trim()&&$('global-hits').children.length)$('global-results').hidden=false;});
  global.addEventListener('input', () => globalRequest.schedule());
  $('global-search').addEventListener('submit', e => {e.preventDefault(); globalSearch();});
  for (const id of ['global-mode','global-regex']) $(id).addEventListener('change', globalSearch);
  $('global-clear').onclick = () => {global.value = ''; globalSearch(); global.focus();};
  global.addEventListener('keydown', e => {if (e.key === 'Escape') {global.value='';globalSearch();}});

  try{const saved=JSON.parse(sessionStorage.getItem('agentbanter.global-search'));if(saved){global.value=saved.q||'';$('global-mode').value=saved.mode||'find';$('global-regex').checked=!!saved.regex;if(global.value)globalSearch(false);}}catch{}

  const taskRequest = requester(taskSearch);
  let liveTimer;
  function resetBlocks() {
    document.querySelectorAll('#timeline [data-search-block],#timeline .step').forEach(node => {
      delete node.dataset.searchHidden; node.classList.remove('search-match','search-current','search-reveal');
    });
  }
  function applyTask(data) {
    const selected=matches[current];
    resetBlocks();
    const ids = new Set(data.anchors), filtering = $('task-mode').value === 'filter';
    const units = [...document.querySelectorAll('#timeline [data-search-block]')];
    matches = units.filter(n => ids.has(n.id)).map(n => n.id); current = selected ? matches.indexOf(selected) : -1;
    if(current>=0)$(selected)?.classList.add('search-current');
    for (const node of units) {
      const match = ids.has(node.id);
      node.classList.toggle('search-match', match); node.classList.toggle('search-reveal', match);
      if (filtering && !match) node.dataset.searchHidden = 'true';
    }
    for (const step of document.querySelectorAll('#timeline .step')) {
      const children = [...step.querySelectorAll('[data-search-block]')];
      step.classList.toggle('search-reveal', children.some(n => ids.has(n.id)));
      if (filtering && children.length && children.every(n => n.dataset.searchHidden)) step.dataset.searchHidden = 'true';
    }
    renderHits($('task-hits'), data);
    $('task-results').hidden = !data.hits.length;
    if(!matches.length&&data.hits.length)$('task-results').open=true;
    $('task-status').textContent = description(data) + ' · ' + matches.length + ' in this pane';
    $('task-previous').disabled = $('task-next').disabled = !matches.length;
  }
  function taskSearch() {
    if (!sid) return;
    taskRequest.cancel();
    active = !!$('task-query').value.trim() || $('task-kind').value !== 'all' || $('task-agent').value !== 'all';
    window.conversationSearchActive = active;
    document.body.classList.toggle('conversation-search-active',active);
    if (!active) {
      matches=[];current=-1;resetBlocks();$('task-hits').replaceChildren();$('task-results').hidden=true;
      $('task-status').textContent='Search includes collapsed blocks and recorded child agents.';
      $('task-previous').disabled=$('task-next').disabled=true;return;
    }
    const follow = $('t-follow'); if (follow) follow.checked = false;
    $('task-status').textContent='Searching this conversation and its recorded agents…';
    taskRequest.request({scope:sid,q:$('task-query').value,kind:$('task-kind').value,agent:$('task-agent').value,regex:$('task-regex').checked,case:$('task-case').checked}, applyTask, error => {
      resetBlocks();matches=[];$('task-hits').replaceChildren();$('task-results').hidden=true;
      $('task-status').textContent=error;$('task-previous').disabled=$('task-next').disabled=true;
    });
  }
  function navigate(direction) {
    if (!matches.length) return;
    current = current < 0 ? (direction < 0 ? matches.length - 1 : 0) : (current + direction + matches.length) % matches.length;
    reveal(matches[current]); $('task-status').textContent='Block '+(current+1)+' of '+matches.length+' matches in this pane. Results include recorded child agents below.';
  }
  if (sid) {
    new ResizeObserver(()=>document.documentElement.style.setProperty('--task-search-height',Math.ceil($('task-search').getBoundingClientRect().height)+'px')).observe($('task-search'));
    $('task-query').addEventListener('input',()=>taskRequest.schedule());
    $('task-search').addEventListener('submit',e=>{e.preventDefault();if(matches.length)navigate(1);else taskSearch();});
    for (const id of ['task-kind','task-agent','task-mode','task-regex','task-case']) $(id).addEventListener('change',taskSearch);
    window.clearConversationSearch=()=>{
      $('task-query').value='';$('task-kind').value='all';$('task-agent').value='all';taskSearch();
    };
    $('task-clear').onclick=()=>{window.clearConversationSearch();$('task-query').focus();};
    $('task-previous').onclick=()=>navigate(-1);$('task-next').onclick=()=>navigate(1);
    $('task-previous').disabled=$('task-next').disabled=true;
    $('task-query').addEventListener('keydown',e=>{
      if(e.key==='Escape')window.clearConversationSearch();
      if(e.key==='Enter'&&e.shiftKey){e.preventDefault();navigate(-1);}
    });
    function updateAgents(members) {
      const select=$('task-agent'), selected=select.value;
      select.replaceChildren(make('option','All agents in this conversation'));select.firstChild.value='all';
      for(const member of members){const option=make('option',member.name+' · '+member.provider+(member.id===sid?' · this pane':''));option.value=member.id;select.append(option);}
      if([...select.options].some(o=>o.value===selected))select.value=selected;
    }
    document.addEventListener('team:updated',e=>updateAgents(e.detail));
    fetch('/s/'+encodeURIComponent(sid)+'/team').then(r=>r.ok?r.json():Promise.reject()).then(data=>updateAgents(data.members)).catch(()=>{$('task-status').textContent+=' Agent list is temporarily unavailable.';});
    document.addEventListener('conversation:updated',()=>{
      if(active&&!liveTimer)liveTimer=setTimeout(()=>{liveTimer=null;taskSearch();},1500);
    });
  }
  document.addEventListener('keydown',e=>{
    if(e.key==='/'&&!e.metaKey&&!e.ctrlKey&&!e.altKey&&!e.target.closest('input,textarea,select,[contenteditable]')){e.preventDefault();global.focus();}
    if((e.ctrlKey||e.metaKey)&&e.key.toLowerCase()==='f'){
      e.preventDefault();(e.shiftKey||!sid?global:$('task-query')).focus();
    }
  });
})();
