(() => {
  const $=id=>document.getElementById(id),root=document.documentElement;
  const stops=[[2200,[255,184,100]],[3000,[255,213,159]],[4000,[255,230,204]],[5000,[255,244,229]],[6500,[230,240,255]]];
  function colour(value){let i=1;while(i<stops.length-1&&value>stops[i][0])i++;const[a,ac]=stops[i-1],[b,bc]=stops[i],r=(value-a)/(b-a);return ac.map((v,j)=>Math.round(v+(bc[j]-v)*r)).join(', ');}
  function stored(key,fallback){try{return localStorage.getItem('agentbanter.'+key)||fallback;}catch{return fallback;}}
  function save(key,value){try{localStorage.setItem('agentbanter.'+key,value);}catch{}}
  function popover(toggle,panel,close){
    function show(open){panel.hidden=!open;toggle.setAttribute('aria-expanded',String(open));if(open)panel.querySelector('select,input')?.focus();}
    toggle.onclick=()=>show(panel.hidden);close.onclick=()=>{show(false);toggle.focus();};
    document.addEventListener('keydown',e=>{if(e.key==='Escape'&&!panel.hidden){show(false);toggle.focus();}});
    document.addEventListener('click',e=>{if(!panel.hidden&&!panel.contains(e.target)&&!toggle.contains(e.target))show(false);});
  }
  const lamp=$('lamp-toggle'),temperature=$('lamp-temperature');
  if(lamp&&temperature){
    function apply(){const k=Math.max(2200,Math.min(6500,Number(temperature.value)||5000));temperature.value=k;root.style.setProperty('--lamp-rgb',colour(k));$('lamp-temperature-value').value=k+' K';lamp.textContent=k+' K ▾';document.querySelectorAll('.lamp-label').forEach(el=>el.textContent='TASK LIGHT · '+k+'K');save('lamp-kelvin',k);}
    temperature.value=stored('lamp-kelvin',5000);temperature.addEventListener('input',apply);apply();popover(lamp,$('lamp-settings'),$('lamp-close'));
  }
  if($('rgb-toggle')){
    const effect=$('rgb-effect'),brightness=$('rgb-brightness'),temperature=$('rgb-temperature');
    effect.value=stored('rgb-effect','rainbow');if(!effect.value)effect.value='rainbow';brightness.value=stored('rgb-brightness',70);temperature.value=stored('rgb-kelvin',5000);
    function apply(){
      const b=Math.max(0,Math.min(100,Number(brightness.value)||0)),k=Math.max(2200,Math.min(6500,Number(temperature.value)||5000));
      root.dataset.rgbEffect=effect.value;root.style.setProperty('--rgb-brightness',String(b/100));root.style.setProperty('--rgb-white',colour(k));
      $('rgb-brightness-value').value=b+'%';$('rgb-temperature-value').value=k+' K';temperature.disabled=effect.value!=='white';
      save('rgb-effect',effect.value);save('rgb-brightness',b);save('rgb-kelvin',k);
    }
    for(const input of[effect,brightness,temperature])input.addEventListener('input',apply);apply();popover($('rgb-toggle'),$('rgb-settings'),$('rgb-close'));
  }
})();
