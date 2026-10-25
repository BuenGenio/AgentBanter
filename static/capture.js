(() => {
  const $ = id=>document.getElementById(id);
  const root=document.querySelector('.capture-workspace');if(!root)return;
  let entries=[], cursor=0, paused=false, running=false, dropped=0, selected=null;
  const text = (tag,value,cls)=>{const e=document.createElement(tag);e.textContent=value;if(cls)e.className=cls;return e;};
  function draw() {
    const query=$('capture-filter').value.toLowerCase(),channel=$('capture-channel').value;
    const filtered=entries.filter(e=>(!channel||e.channel===channel)&&(!query||JSON.stringify(e).toLowerCase().includes(query)));
    const visible=filtered.slice(-300); const fragment=document.createDocumentFragment();
    for(const e of visible){
      const row=document.createElement('tr'); row.className=e.seq===selected?'selected':'';
      const button=text('button',e.seq);button.type='button';button.setAttribute('aria-label','Inspect record '+e.seq);
      const select=()=>{selected=e.seq; $('record-label').textContent='#'+e.seq+' / '+e.kind;window.renderInspector?.($('record-inspector'),e);draw();};
      button.onclick=select; row.onclick=event=>{if(event.target!==button)select();};
      const cell=document.createElement('td');cell.append(button);row.append(cell,text('td',e.time.slice(11,23)),text('td',e.channel,'channel-'+e.channel),text('td',e.kind),text('td',e.detail?.endpoint||e.detail?.path||e.session||e.detail?.message||e.detail?.summary||'AgentBanter'));fragment.append(row);
    }
    const scroll=$('capture-rows').parentElement.parentElement;const nearBottom=scroll.scrollHeight-scroll.scrollTop-scroll.clientHeight<60;
    $('capture-rows').replaceChildren(fragment);if(nearBottom)scroll.scrollTop=scroll.scrollHeight;
    $('capture-empty').hidden=visible.length>0;
    $('capture-count').textContent=`${filtered.length} matched · ${entries.length} retained${filtered.length>300?' · latest 300 shown':''}${dropped?' · '+dropped+' evicted on server':''}`;
  }
  async function poll(){
    if(root.dataset.enabled!=='true')return;
    try{
      if(!paused){
        const response=await fetch('/capture/records?after='+cursor);if(!response.ok)throw new Error('Capture unavailable: '+response.status);
        const data=await response.json();cursor=data.next;running=data.running;dropped=data.dropped;
        entries.push(...data.entries);entries=entries.slice(-4000);
        for(const e of data.entries.filter(e=>e.channel==='packet'))$('packet-status').textContent=e.kind==='packet.summary'?'Packet summaries are arriving.':e.detail.message||e.detail.reason||e.kind;
        $('capture-state').textContent=running?'● Capturing':'Stopped';$('capture-start').disabled=running;$('capture-stop').disabled=!running;
        draw();
      }
    }catch(error){$('capture-state').textContent=error.message;}
    finally{setTimeout(poll,1000);}
  }
  async function control(action){
    try{
      const response=await fetch('/capture/control',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({action})});
      if(!response.ok)throw new Error('Capture command failed: '+response.status);
      const data=await response.json();running=data.running;
      if(action==='clear'){entries=[];selected=null;$('record-inspector').textContent='Select a record to inspect its fields.';draw();}
      if(action==='start')$('packet-status').textContent='Attempting OS packet capture…';
      $('capture-state').textContent=running?'● Capturing':'Stopped';$('capture-start').disabled=running;$('capture-stop').disabled=!running;
    }catch(error){$('capture-state').textContent=error.message;}
  }
  $('capture-start').onclick=()=>control('start');$('capture-stop').onclick=()=>control('stop');$('capture-clear').onclick=()=>control('clear');
  $('capture-pause').onclick=()=>{paused=!paused;$('capture-pause').textContent=paused?'Resume view':'Pause view';if(paused)$('capture-state').textContent='View paused · capture continues';};
  $('capture-filter').oninput=draw;$('capture-channel').onchange=draw;
  $('capture-export').onclick=()=>{const blob=new Blob([JSON.stringify({exported_at:new Date().toISOString(),dropped,records:entries},null,2)],{type:'application/json'});const url=URL.createObjectURL(blob);const a=document.createElement('a');a.href=url;a.download='agentbanter-capture.json';a.click();setTimeout(()=>URL.revokeObjectURL(url),1000);};
  poll();
})();
