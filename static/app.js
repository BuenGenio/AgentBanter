// Header: hides on scroll down, returns on scroll up; always shown near the top,
// for ~900ms after load, and while the page scrolls itself (follow mode).
const navGrace = (() => {
  const hdr = document.querySelector('.top');
  let graceUntil = performance.now() + 900;
  const grace = ms => { graceUntil = Math.max(graceUntil, performance.now() + ms); };
  if (!hdr) return grace;
  let lastY = window.scrollY;
  window.addEventListener('scroll', () => {
    const y = window.scrollY, delta = y - lastY;
    lastY = y;
    if (y < 96 || performance.now() < graceUntil) { hdr.classList.remove('is-hidden'); return; }
    if (delta > 6) hdr.classList.add('is-hidden');
    else if (delta < -6) hdr.classList.remove('is-hidden');
  }, { passive: true });
  hdr.addEventListener('focusin', () => hdr.classList.remove('is-hidden'));
  return grace;
})();

// Applies server-rendered fragments pushed over SSE. Kinds: block, tool, tool_result, decision, stats.
(() => {
  const tl = document.getElementById('timeline');
  if (!tl) return;
  const sid = tl.dataset.session;
  let seq = parseInt(tl.dataset.seq || '0', 10);
  const live = document.getElementById('live');
  const dec = document.getElementById('decisions');
  const dcount = document.getElementById('dcount');
  const $ = id => document.getElementById(id);

  // View toggles → body classes (CSS does the hiding).
  const toggles = { 't-reasoning': 'hide-reasoning', 't-tools': 'hide-tools', 't-inferred': 'hide-inferred' };
  for (const [id, cls] of Object.entries(toggles)) {
    const el = $(id);
    if (!el) continue;
    const key = 'agentbanter.' + id;
    try { const saved = localStorage.getItem(key); if (saved !== null) el.checked = saved === '1'; } catch {}
    const apply = () => {
      document.body.classList.toggle(cls, !el.checked);
      try { localStorage.setItem(key, el.checked ? '1' : '0'); } catch {}
    };
    el.addEventListener('change', apply);
    apply();
  }
  const following = () => $('t-follow')?.checked;

  // Small screens: decisions start collapsed so the timeline is reachable.
  const dpanel = $('dpanel');
  if (dpanel && window.matchMedia('(max-width: 1000px)').matches) dpanel.removeAttribute('open');

  const frag = html => { const t = document.createElement('template'); t.innerHTML = html.trim(); return t.content; };

  function apply(m) {
    switch (m.kind) {
      case 'reset': location.reload(); return;
      case 'tool': {
        const step = m.step && $(m.step)?.querySelector('.tools');
        (step || tl).appendChild(frag(m.html));
        break;
      }
      case 'tool_result': {
        const li = $('tool-' + m.tool);
        if (!li) return;
        const slot = li.querySelector('[data-slot]');
        if (slot) slot.innerHTML = m.html;
        const rd = li.querySelector('[data-rslot]');
        if (rd) { rd.replaceChildren(); window.renderInspector?.(rd, m.raw || m.detail || '', m.detail || ''); }
        li.classList.add(m.ok ? 'ok' : 'err');
        break;
      }
      case 'decision':
        tl.appendChild(frag(m.html));
        dec.appendChild(frag(m.panel));
        break;
      case 'stats': {
        if (m.prompt) {
          const c = $('chips-' + m.prompt);
          if (c && m.chips != null) {
            const expanded = c.querySelector('.activity-details')?.open;
            c.innerHTML = m.chips;
            const details = c.querySelector('.activity-details');
            if (details && expanded) details.open = true;
          }
        }
        const h = $('hdr-stats'); if (h) h.innerHTML = m.header;
        if (dcount && m.dcount != null) dcount.textContent = m.dcount;
        return;
      }
      default:
        tl.appendChild(frag(m.html));
    }
    window.enhanceCode?.(tl);
    document.dispatchEvent(new Event('conversation:updated'));
    if (following() && !window.conversationSearchActive) {
      navGrace(1200);
      window.scrollTo({ top: document.documentElement.scrollHeight, behavior: 'smooth' });
    }
  }

  // EventSource reconnects on its own and sends Last-Event-ID, which the server honours over `from`.
  const es = new EventSource(`/s/${sid}/events?from=${seq}&generation=${tl.dataset.generation || 0}`);
  es.onopen = () => live.classList.add('on');
  es.onerror = () => live.classList.remove('on');
  es.onmessage = e => {
    let m; try { m = JSON.parse(e.data); } catch { return; }
    apply(m);
    if (e.lastEventId) seq = parseInt(e.lastEventId, 10) + 1;
  };
})();

// Team membership is supplied by recorded parent/child metadata, not shared cwd.
(() => {
  const panel=document.getElementById('team-panel');if(!panel)return;
  const form=document.getElementById('composer'), select=form?.elements.recipient, box=form?.elements.text;
  const button=form?.querySelector('button'), status=document.getElementById('composer-status');
  let members=[], busy=false, batch=[];
  const expandedAgents=new Set();
  const node=(tag,text)=>{const el=document.createElement(tag);el.textContent=text;return el;};
  const selected=()=>select?.value==='@team'?members.filter(m=>m.sendable):members.filter(m=>m.id===select?.value&&m.sendable);
  const lock=value=>{busy=value;if(form){button.disabled=value||!selected().length;select.disabled=value;box.readOnly=value;}};
  function selection(){
    if(!form)return;
    const targets=selected(), all=select.value==='@team';
    document.getElementById('delivery-mode').textContent=all
      ?'Send separately to '+targets.map(m=>m.name).join(', ')+'. Transcript-only members are excluded.'
      :(targets[0]?.mode||'No direct delivery available.');
    const link=document.getElementById('recipient-link');link.hidden=all||!targets.length;
    if(targets.length)link.href='/s/'+encodeURIComponent(targets[0].id);
    button.textContent=all?'Send to '+targets.length+' team members':targets[0]?.provider==='Codex'?'Queue message':'Continue Claude';
    lock(busy);
  }
  async function refresh(){
    if(!document.hidden&&!busy){
      try{
        const response=await fetch('/s/'+encodeURIComponent(panel.dataset.session)+'/team');
        if(!response.ok)throw Error('Team unavailable');
        const data=await response.json();members=data.members;
        document.getElementById('team-note').textContent=data.note;
        document.dispatchEvent(new CustomEvent('team:updated',{detail:data.members}));
        const list=document.getElementById('team-members');list.replaceChildren();
        const count=document.getElementById('agent-count');if(count)count.textContent=members.length;
        for(const m of members){
          const item=node('li',''), detail=node('details',''), summary=node('summary','');
          detail.className='agent-card';detail.open=expandedAgents.has(m.id);
          detail.addEventListener('toggle',()=>{if(detail.open)expandedAgents.add(m.id);else expandedAgents.delete(m.id);});
          summary.append(node('strong',m.name),node('small',m.provider+' · '+(m.recent_activity?'Recent activity':'Previously recorded')));
          detail.append(summary);
          if(m.task!==m.name&&m.task!==m.id)detail.append(node('p',m.task));
          detail.append(node('small',m.parent_name?'Child of '+m.parent_name:'Conversation lead'),
            node('small','Last recorded activity: '+(m.last_activity?new Date(m.last_activity).toLocaleString():m.age)),
            node('small','Conversation ID: '+m.id));
          if(m.cwd)detail.append(node('small','Working directory: '+m.cwd));
          detail.append(node('small','Relationship evidence: '+m.evidence));
          const link=node('a','View conversation →');link.href='/s/'+encodeURIComponent(m.id);detail.append(link);
          const message=node('button','Message directly');message.type='button';
          message.disabled=!form||!m.sendable;
          message.title=!form?'Messaging is disabled for this session':m.mode;
          message.addEventListener('click',()=>{
            select.value=m.id;selection();
            const composer=document.getElementById('message-instrument');if(composer)composer.open=true;
            box.focus();box.scrollIntoView({block:'nearest'});
          });
          detail.append(message);
          if(!m.sendable||!form)detail.append(node('p',!form?'Messaging is disabled for this session.':m.mode));
          const parent=members.find(p=>p.id===m.parent_id&&p.sendable);
          if(form&&parent){
            const relay=node('button','Message parent');relay.type='button';
            relay.addEventListener('click',()=>{
              select.value=parent.id;selection();
              const composer=document.getElementById('message-instrument');if(composer)composer.open=true;
              box.focus();
            });detail.append(relay);
          }
          item.append(detail);list.append(item);
        }
        if(form){
          const previous=select.value;select.replaceChildren();
          for(const m of members){
            const option=node('option',m.provider+' · '+m.name+(m.sendable?'':' (transcript only)'));
            option.value=m.id;option.disabled=!m.sendable;select.append(option);
          }
          if(members.filter(m=>m.sendable).length>1){const option=node('option','All reachable team members');option.value='@team';select.append(option);}
          const fallback=members.find(m=>m.id===panel.dataset.session&&m.sendable)||members.find(m=>m.sendable);
          select.value=[...select.options].some(o=>o.value===previous&&!o.disabled)?previous:(fallback?.id||'');
          selection();
        }
      }catch(error){document.getElementById('team-note').textContent=error.message+' · last known team retained';}
    }
    setTimeout(refresh,10000);
  }
  function receipts(){
    status.replaceChildren();
    for(const entry of batch){
      const row=node('p',entry.name+': '+entry.message);
      if(entry.session){const link=node('a',entry.session===entry.id?' View conversation →':' Open continuation →');link.href='/s/'+encodeURIComponent(entry.session);row.append(link);}
      status.append(row);
    }
  }
  function apply(entry,receipt){
    entry.state=receipt.state;entry.session=receipt.session;
    entry.message=receipt.state==='queued'?'Queued; reply pending.':receipt.state==='done'?'Response completed.':receipt.state==='running'?'Submitted; working…':receipt.error||'Delivery failed.';
  }
  async function poll(){
    for(const entry of batch.filter(e=>e.state==='running')){
      try{
        const response=await fetch('/s/'+encodeURIComponent(entry.id)+'/send');
        if(!response.ok)throw Error('Cannot check receipt');
        apply(entry,await response.json());
      }catch{entry.state='uncertain';entry.message='Status unavailable. Check conversation before retrying.';}
    }
    receipts();
    if(batch.some(e=>e.state==='running'))setTimeout(poll,1500);
    else{
      lock(false);
      if(batch.every(e=>['queued','done'].includes(e.state)))box.value='';
      else status.append(node('p','Draft retained. Some recipients may already have received it; select only failed recipients before retrying.'));
    }
  }
  if(form){
    select.addEventListener('change',selection);
    form.addEventListener('submit',async event=>{
      event.preventDefault();if(busy||!box.value.trim()||!selected().length)return;
      // Freeze the displayed recipient list for this submission; never add new agents mid-broadcast.
      batch=selected().map(m=>({id:m.id,name:m.name,state:'pending',message:'Waiting to submit…'}));
      const text=box.value.trim();lock(true);receipts();
      for(const entry of batch){
        try{
          const response=await fetch('/s/'+encodeURIComponent(entry.id)+'/send',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({text})});
          const receipt=await response.json();
          if(!response.ok)throw Error(receipt.error||'Submission rejected');
          apply(entry,receipt);
        }catch(error){entry.state='failed';entry.message=error.message+' Check delivery before retrying.';}
        receipts();
      }
      poll();
    });
    button.disabled=true;
  }
  refresh();
})();

// Process references and shared tool permalinks reveal their exact timeline entry.
(() => {
  function reveal(hash) {
    let id; try { id=decodeURIComponent(hash.slice(1)); } catch { return; }
    if(!id.startsWith('tool-'))return;
    const target=document.getElementById(id);
    if(!target || !target.closest('#timeline'))return;
    window.clearConversationSearch?.();
    const follow=document.getElementById('t-follow');if(follow)follow.checked=false;
    const tools=document.getElementById('t-tools');
    if(tools){tools.checked=true;tools.dispatchEvent(new Event('change'));}
    document.body.classList.remove('hide-tools');
    for(let parent=target;parent;parent=parent.parentElement){
      if(parent.tagName==='DETAILS')parent.open=true;
    }
    const detail=target.querySelector('details');if(detail)detail.open=true;
    document.querySelectorAll('.conversation-reference-target').forEach(el=>el.classList.remove('conversation-reference-target'));
    target.classList.add('conversation-reference-target');
    target.setAttribute('tabindex','-1');target.focus({preventScroll:true});
    target.scrollIntoView({block:'center',behavior:matchMedia('(prefers-reduced-motion: reduce)').matches?'instant':'smooth'});
  }
  document.addEventListener('click',event=>{
    const link=event.target.closest('a.process-conversation-link');
    if(!link || event.ctrlKey || event.metaKey || event.shiftKey || event.altKey)return;
    event.preventDefault();history.replaceState(null,'',link.hash);reveal(link.hash);
  });
  window.addEventListener('hashchange',()=>reveal(location.hash));
  if(location.hash)requestAnimationFrame(()=>reveal(location.hash));
})();


// Remember each instrument independently, without changing agent-card expansion.
(() => {
  const session=document.getElementById('timeline')?.dataset.session;if(!session)return;
  document.querySelectorAll('details.instrument[id]').forEach(panel=>{
    const key='agentbanter.instrument.'+session+'.'+panel.id;
    try{const saved=localStorage.getItem(key);if(saved!==null)panel.open=saved==='1';}catch{}
    panel.addEventListener('toggle',()=>{try{localStorage.setItem(key,panel.open?'1':'0');}catch{}});
  });
})();
