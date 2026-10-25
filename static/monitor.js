(() => {
  const root=document.getElementById('process-monitor');if(!root)return;
  const $=id=>document.getElementById(id), node=(tag,cls,text)=>{const el=document.createElement(tag);if(cls)el.className=cls;if(text!==undefined)el.textContent=text;return el;};
  let snapshot={processes:[]}, selected=null;
  const expanded=new Set();
  function socketMatches(socket,kind){
    if(kind==='listeners')return socket.state==='LISTEN';
    if(kind==='connections')return socket.state==='ESTABLISHED';
    if(['TCP','UDP','IPC'].includes(kind))return socket.transport===kind;
    return true;
  }
  function render(){
    const query=$('process-search').value.toLowerCase(),kind=$('process-kind').value,scope=$('process-scope').value;
    const matches=snapshot.processes.filter(p=>{
      if(scope==='session'&&p.attribution==='project'||scope==='project'&&p.attribution!=='project')return false;
      if(kind==='no-sockets'&&p.sockets.length)return false;
      if(kind!=='all'&&kind!=='no-sockets'&&!p.sockets.some(s=>socketMatches(s,kind)))return false;
      return !query||JSON.stringify({pid:p.pid,name:p.name,cwd:p.cwd,sockets:p.sockets}).toLowerCase().includes(query);
    });
    $('process-count').textContent=matches.length;
    const fragment=document.createDocumentFragment();
    for(const p of matches){
      const card=node('details','process-card');card.open=expanded.has(p.pid);
      card.addEventListener('toggle',()=>{if(card.open)expanded.add(p.pid);else expanded.delete(p.pid);});
      const summary=node('summary');
      const title=node('div','process-title');title.append(node('span','status-dot'),node('strong','',p.name),node('span','process-pid','#'+p.pid));
      const attribution=node('span','process-attribution '+p.attribution,p.attribution==='project'?'Project candidate':p.attribution==='descendant'?'Session descendant':'Session linked');
      const badges=node('div','port-badges');
      const listens=[...new Set(p.sockets.filter(s=>s.state==='LISTEN').map(s=>s.port).filter(Boolean))];
      listens.forEach(port=>badges.append(node('span','port-badge',':'+port)));
      if(!listens.length)badges.append(node('span','process-socket-count',p.sockets.length?p.sockets.length+' sockets':'No open sockets'));
      summary.append(title,attribution,badges);card.append(summary);
      const body=node('div','process-body');
      const stats=node('dl','process-stats');
      for(const [label,value]of [['CPU',p.cpu_percent.toFixed(1)+'%'],['Memory',(p.memory_kib/1024).toFixed(1)+' MiB'],['Running',p.elapsed],['Parent PID',p.ppid]]){const item=node('div');item.append(node('dt','',label),node('dd','',value));stats.append(item);}
      body.append(stats,node('div','process-path',p.executable),node('div','process-cwd',p.cwd||'Working directory unavailable'),node('p','process-evidence',p.evidence));
      if(p.conversation_link){
        const link=node('a','process-conversation-link','View in conversation →');
        link.href='#'+encodeURIComponent(p.conversation_link.anchor);
        link.title=p.conversation_link.evidence;
        body.append(link,node('p','process-reference-note',p.conversation_link.evidence));
      }else{
        body.append(node('p','process-reference-note','No matching conversation entry recorded.'));
      }
      for(const socket of p.sockets.filter(s=>socketMatches(s,kind))){
        const item=node('div','socket-card');item.append(node('span','socket-protocol',socket.transport||socket.family),node('span','socket-state',socket.state||'Open'),node('code','socket-endpoint',socket.endpoint));
        if(socket.state==='LISTEN'&&socket.port){
          const link=node('a','socket-open','Open HTTP ↗');link.href=`http://${socket.endpoint.startsWith('[::1]')?'[::1]':'127.0.0.1'}:${socket.port}/`;link.target='_blank';link.rel='noopener noreferrer';link.title='Try this listening port in a browser; the service may use another protocol.';item.append(link);
        }
        body.append(item);
      }
      const inspect=node('button','process-inspect','Inspect snapshot');inspect.type='button';
      const existing=selected===p.pid ? $('process-list').querySelector(`.process-object[data-pid="${p.pid}"]`) : null;
      const data=existing||node('div','process-object');data.dataset.pid=p.pid;data.hidden=selected!==p.pid;
      inspect.onclick=()=>{selected=selected===p.pid?null:p.pid;data.hidden=selected!==p.pid;if(!data.hidden)window.renderInspector?.(data,p);};
      if(!data.hidden&&!existing)window.renderInspector?.(data,p);
      body.append(inspect,data);card.append(body);fragment.append(card);
    }
    if(!matches.length)fragment.append(node('p','process-empty',snapshot.processes.length?'No processes match these filters.':'No associated processes visible.'));
    $('process-list').replaceChildren(fragment);
  }
  async function refresh(){
    if(document.hidden){setTimeout(refresh,5000);return;}
    try{
      const response=await fetch(`/s/${encodeURIComponent(root.dataset.session)}/processes`);if(!response.ok)throw Error('Process monitor unavailable ('+response.status+')');
      snapshot=await response.json();
      const errors=snapshot.warnings||[];
      $('process-status').textContent=errors.length?errors.join(' · '):'Live · every 5 seconds · '+new Date(snapshot.observed_at).toLocaleTimeString();
      if(snapshot.note)$('process-note').textContent=snapshot.note;
      render();
    }catch(error){$('process-status').textContent=error.message+' · last snapshot retained';}
    setTimeout(refresh,5000);
  }
  $('process-search').addEventListener('input',render);$('process-kind').addEventListener('change',render);$('process-scope').addEventListener('change',render);
  refresh();
})();
