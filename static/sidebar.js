(() => {
  const search = document.getElementById('session-search');
  const list = document.querySelector('.conversation-list');
  if (!search || !list) return;
  const rows = [...list.querySelectorAll('.conversation')];
  search.addEventListener('input', () => {
    const query = search.value.trim().toLocaleLowerCase();
    let count = 0;
    for (const row of rows) {
      row.hidden = !row.dataset.search.toLocaleLowerCase().includes(query);
      if (!row.hidden) count++;
    }
    list.querySelectorAll('.conversation-group').forEach(group => {
      group.hidden = ![...group.querySelectorAll('.conversation')].some(row => !row.hidden);
    });
    document.getElementById('search-empty').hidden = count > 0;
  });
  document.addEventListener('keydown', event => {
    if (event.key === '/' && !event.metaKey && !event.ctrlKey && !event.altKey && !event.target.closest('input, textarea, [contenteditable]')) {
      event.preventDefault(); search.focus();
    }
  });
  try {
    list.scrollTop = Number(sessionStorage.getItem('agentbanter.sidebar-scroll') || 0);
    list.addEventListener('scroll', () => sessionStorage.setItem('agentbanter.sidebar-scroll', list.scrollTop), { passive: true });
  } catch {}
})();

// Task-light temperature changes the emitter and the light projected onto the mat.
(() => {
  const toggle=document.getElementById('lamp-toggle'),panel=document.getElementById('lamp-settings'),slider=document.getElementById('lamp-temperature'),output=document.getElementById('lamp-temperature-value');
  if(!toggle||!panel||!slider)return;
  const stops=[[2200,[255,184,100]],[3000,[255,213,159]],[4000,[255,230,204]],[5000,[255,244,229]],[6500,[230,240,255]]];
  function apply(value){
    const kelvin=Math.max(2200,Math.min(6500,Number(value)||5000));
    let i=1;while(i<stops.length-1&&kelvin>stops[i][0])i++;
    const [a,ac]=stops[i-1],[b,bc]=stops[i],ratio=(kelvin-a)/(b-a);
    const rgb=ac.map((v,j)=>Math.round(v+(bc[j]-v)*ratio));
    document.documentElement.style.setProperty('--lamp-rgb',rgb.join(', '));
    output.value=`${kelvin} K`;toggle.textContent=`${kelvin} K ▾`;slider.value=kelvin;slider.setAttribute('aria-valuetext',`${kelvin} kelvin`);
    document.querySelectorAll('.lamp-label').forEach(label=>label.textContent=`TASK LIGHT · ${kelvin}K`);
    try{localStorage.setItem('agentbanter.lamp-kelvin',kelvin);}catch{}
  }
  function show(open){panel.hidden=!open;toggle.setAttribute('aria-expanded',String(open));if(open)slider.focus();}
  toggle.onclick=()=>show(panel.hidden);document.getElementById('lamp-close').onclick=()=>{show(false);toggle.focus();};
  document.addEventListener('keydown',event=>{if(event.key==='Escape'&&!panel.hidden){show(false);toggle.focus();}});
  document.addEventListener('click',event=>{if(!panel.hidden&&!panel.contains(event.target)&&!toggle.contains(event.target))show(false);});
  slider.addEventListener('input',()=>apply(slider.value));
  let initial=5000;try{initial=localStorage.getItem('agentbanter.lamp-kelvin')||5000;}catch{}apply(initial);
})();
