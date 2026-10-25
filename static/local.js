(() => {
  const $=id=>document.getElementById(id), root=$('local-workbench');if(!root)return;
  const make=(tag,text,cls)=>{const node=document.createElement(tag);if(text!==undefined)node.textContent=text;if(cls)node.className=cls;return node;};
  let session=new URL(location.href).searchParams.get('session')||'',selected=new URL(location.href).searchParams.get('trace')||'',busy=false,controller,latest,rawId='',listKey='',toolsKey='';
  const enabled=root.dataset.enabled==='true';
  $('local-native-url').textContent=location.origin+'/ollama';$('local-openai-url').textContent=location.origin+'/ollama/v1';
  for(const button of document.querySelectorAll('[data-copy]'))button.onclick=async()=>{try{await navigator.clipboard.writeText($(button.dataset.copy).textContent);button.textContent='Copied';}catch{button.textContent='Select and copy the address';}};
  async function api(path,options){const response=await fetch(path,options);const data=await response.json();if(!response.ok)throw Error(data.error||'Request failed');return data;}
  const post=(path,body,signal)=>api(path,{method:'POST',headers:{'content-type':'application/json'},body:JSON.stringify(body),signal});
  function lock(value){busy=value;document.querySelectorAll('#local-recordings button').forEach(button=>button.disabled=value);$('local-send').disabled=value||!enabled||!$('local-model').value;$('local-stop').disabled=!value;$('local-new').disabled=value;$('local-model').disabled=value;$('local-think').disabled=value;$('local-refresh').disabled=value;$('local-analyze').disabled=value||!latest?.thinking||latest?.status==='running'||!enabled;}
  async function modelInfo(){
    const model=$('local-model').value;if(!model)return;
    try{const data=await post('/local/model',{model});if($('local-model').value!==model)return;
      $('local-think').replaceChildren(make('option','Model default'));$('local-think').firstChild.value='null';
      const values=data.thinking?.values|| (data.capabilities?.includes('thinking')?[false,true]:[]);
      for(const value of values){const option=make('option',typeof value==='boolean'?(value?'On':'Off'):value);option.value=JSON.stringify(value);$('local-think').append(option);}
      const capabilities=(data.capabilities||[]).join(' · ')||'Capabilities not reported';
      $('local-model-info').textContent=model+' · '+capabilities+(data.details?.runner?' · '+data.details.runner:'');
    }catch(error){$('local-model-info').textContent=error.message;}
  }
  async function models(){try{const data=await api('/local/models'),previous=$('local-model').value;$('local-model').replaceChildren();
      for(const model of data.models||[]){const option=make('option',model.name+' · '+(model.size/1e9).toFixed(1)+' GB');option.value=model.name;$('local-model').append(option);}
      if([...$('local-model').options].some(o=>o.value===previous))$('local-model').value=previous;
      if(!$('local-model').options.length){$('local-model-info').textContent='No models installed in Ollama.';}else await modelInfo();lock(busy);
    }catch(error){$('local-model-info').textContent=error.message;$('local-send').disabled=true;}}
  $('local-refresh').onclick=models;$('local-model').onchange=modelInfo;
  function linkConversation(){const link=$('local-conversation');link.hidden=!session;if(session)link.href='/s/'+encodeURIComponent(session);}
  function reset(){analysisKey='';session='';selected='';latest=null;rawId='';toolsKey='';history.replaceState(null,'','/local');$('local-thinking-text').textContent='Thinking will appear here when the model exposes it.';$('local-answer-text').textContent='The answer will appear separately.';$('local-analysis').replaceChildren();$('local-analysis-status').textContent='';$('local-raw').replaceChildren(make('p','Select a recording to inspect its fields.'));$('local-state').textContent='New chat';$('local-tools').hidden=true;$('local-metrics').replaceChildren(make('dt','Status'),make('dd','No trace selected'));linkConversation();lock(false);}
  $('local-new').onclick=reset;$('local-stop').onclick=()=>controller?.abort();
  const ms=value=>value===null||value===undefined?'Not reported':(value/1000).toFixed(3)+' s';
  function metric(label,value){$('local-metrics').append(make('dt',label),make('dd',String(value)));}
  function drawTrace(trace){
    if(latest?.id!==trace.id){analysisKey='';$('local-analysis').replaceChildren();$('local-analysis-status').textContent='';}
    latest=trace;selected=trace.id;session=trace.session;linkConversation();
    if(!busy&&$('local-model').value!==trace.model&&[...$('local-model').options].some(option=>option.value===trace.model)){$('local-model').value=trace.model;modelInfo();}
    $('local-metrics').replaceChildren();metric('Status',trace.status);metric('Model',trace.model);metric('Origin',trace.origin);metric('Elapsed',ms(trace.elapsed_ms));metric('First thinking',ms(trace.first_thinking_ms));metric('First answer',ms(trace.first_answer_ms));
    if(trace.first_thinking_ms!==null&&trace.last_thinking_ms!==null)metric('Thinking stream span',ms(trace.last_thinking_ms-trace.first_thinking_ms));
    metric('Stream records',trace.chunks);metric('Thinking characters',[...trace.thinking].length);metric('Answer characters',[...trace.answer].length);
    const m=trace.metrics||{};
    for(const [key,label] of [['load_duration','Model load'],['prompt_eval_duration','Prompt processing'],['eval_duration','Generation']])if(m[key]!==undefined)metric(label,ms(m[key]/1e6));
    if(m.prompt_eval_count!==undefined)metric('Prompt tokens',m.prompt_eval_count);if(m.eval_count!==undefined)metric('Generated tokens · API total',m.eval_count);
    if(m.eval_duration>0&&m.eval_count!==undefined)metric('Generation rate',(m.eval_count*1e9/m.eval_duration).toFixed(1)+' tokens/s');
    if(m.usage)for(const [key,value] of Object.entries(m.usage))if(typeof value==='number')metric(key.replaceAll('_',' '),value);
    if(m.done_reason)metric('Completion reason',m.done_reason);
    if(trace.preview_truncated)metric('Preview limit','Some fields were shortened');if(trace.unparsed_records)metric('Unparsed records',trace.unparsed_records);
    if(trace.recording_error)metric('Recording error',trace.recording_error);if(trace.error)metric('Response issue',trace.error);
    if(!busy){const thinking=trace.thinking||'No thinking text was exposed in this response.',answer=trace.answer||'No final answer was recorded.';if($('local-thinking-text').textContent!==thinking)$('local-thinking-text').textContent=thinking;if($('local-answer-text').textContent!==answer)$('local-answer-text').textContent=answer;$('local-state').textContent=trace.status;}
    const key=JSON.stringify(trace.tool_calls||[]);if(toolsKey!==key){toolsKey=key;$('local-tools').hidden=!trace.tool_calls?.length;if(trace.tool_calls?.length)window.renderInspector?.($('local-tool-tree'),trace.tool_calls);}
    if(rawId!==trace.id){rawId=trace.id;window.renderInspector?.($('local-raw'),trace);}
    if(trace.analysis)drawAnalysis(trace.analysis);
    $('local-analyze').disabled=busy||!trace.thinking||trace.status==='running'||!enabled;
  }
  function quote(text){const source=$('local-thinking-text'),where=source.textContent.indexOf(text);if(!text||where<0)return;const whole=source.textContent,mark=make('mark',text,'local-evidence');source.replaceChildren(document.createTextNode(whole.slice(0,where)),mark,document.createTextNode(whole.slice(where+text.length)));$('local-thinking').open=true;mark.scrollIntoView({block:'center',behavior:matchMedia('(prefers-reduced-motion: reduce)').matches?'instant':'smooth'});}
  let analysisKey='';
  function drawAnalysis(result){
    const key=JSON.stringify(result);if(analysisKey===key)return;analysisKey=key;
    const host=$('local-analysis');host.replaceChildren();const data=result.analysis||{};
    host.append(make('p',data.summary||'No summary returned.'));
    const steps=make('ol');for(const item of data.steps||[]){const li=make('li');li.append(make('p',item.claim||''));if(item.evidence_quote){const citation=make('button',(item.quote_verified?'Verified source quote: ':'Unverified quote: ')+item.evidence_quote,item.quote_verified?'local-quote':'local-unverified');citation.type='button';citation.disabled=!item.quote_verified;citation.onclick=()=>quote(item.evidence_quote);li.append(citation);}steps.append(li);}if(steps.children.length)host.append(steps);
    for(const [key,title] of [['assumptions','Assumptions'],['alternatives','Alternatives considered'],['issues','Questions and possible gaps']]){const values=data[key];if(!Array.isArray(values)||!values.length)continue;const section=make('details');section.open=true;section.append(make('summary',title));const list=make('ul');for(const value of values)list.append(make('li',typeof value==='string'?value:JSON.stringify(value)));section.append(list);host.append(section);}
    if(data.limits)host.append(make('p',data.limits,'local-notice'));
    $('local-analysis-status').textContent='Generated afterward by '+result.model+' · '+new Date(result.created_at).toLocaleString()+(result.input_shortened?' · Input was shortened':'');
  }
  $('local-analyze').onclick=async()=>{if(!selected)return;const id=selected;$('local-analyze').disabled=true;$('local-analysis-status').textContent='Analyzing the recorded thinking locally…';try{const result=await post('/local/traces/'+encodeURIComponent(id)+'/analysis',{});if(selected===id)drawAnalysis(result);}catch(error){$('local-analysis-status').textContent=error.message;}finally{$('local-analyze').disabled=busy||!latest?.thinking||!enabled;}};
  const rawPanel=$('local-raw').parentElement;const refreshRaw=make('button','Refresh snapshot');refreshRaw.type='button';refreshRaw.onclick=()=>{if(latest)window.renderInspector?.($('local-raw'),latest);};rawPanel.insertBefore(refreshRaw,$('local-raw'));
  rawPanel.insertBefore(make('p','Field inspection uses a snapshot so expanded nodes stay in place.'),$('local-raw'));
  async function loadTrace(id){try{const data=await api('/local/traces/'+encodeURIComponent(id));if(selected&&selected!==id)return;drawTrace(data);}catch(error){$('local-state').textContent=error.message;}}
  async function recordings(){if(document.hidden)return;try{const data=await api('/local/traces'),key=JSON.stringify(data.traces);if(key!==listKey){listKey=key;$('local-recordings').replaceChildren();for(const trace of data.traces){const li=make('li'),button=make('button',trace.model+' · '+trace.status);button.type='button';button.disabled=busy;button.onclick=()=>{if(busy)return;selected=trace.id;analysisKey='';history.replaceState(null,'','/local?trace='+encodeURIComponent(trace.id));loadTrace(trace.id);};li.append(button,make('small',trace.origin+' · '+trace.prompt));$('local-recordings').append(li);}}
      if(!selected&&session){const match=data.traces.find(t=>t.session===session);if(match)selected=match.id;}
      if(selected)await loadTrace(selected);
    }catch(error){$('local-recordings-note').textContent=error.message;}}
  $('local-chat').onsubmit=async event=>{event.preventDefault();if(busy||!enabled)return;const prompt=$('local-prompt').value.trim();if(!prompt)return;
    controller=new AbortController();lock(true);latest=null;selected='';rawId='';analysisKey='';toolsKey='';$('local-thinking-text').textContent='';$('local-answer-text').textContent='';$('local-analysis').replaceChildren();$('local-analysis-status').textContent='';$('local-state').textContent='Waiting for Ollama…';
    let buffer='',reader;const decoder=new TextDecoder();
    function frame(line){if(!line.trim())return;let data;try{data=JSON.parse(line);}catch{return;}if(data.error)throw Error(typeof data.error==='string'?data.error:JSON.stringify(data.error));if(data.message?.thinking){$('local-thinking-text').append(document.createTextNode(data.message.thinking));$('local-state').textContent='Thinking…';}if(data.message?.content){$('local-answer-text').append(document.createTextNode(data.message.content));$('local-state').textContent='Answering…';}if(data.done)$('local-state').textContent='Complete';}
    try{const response=await fetch('/local/chat',{method:'POST',headers:{'content-type':'application/json'},body:JSON.stringify({model:$('local-model').value,text:prompt,session:session||null,think:JSON.parse($('local-think').value)}),signal:controller.signal});if(!response.ok){let data;try{data=await response.json();}catch{data={error:'Local request failed'};}throw Error(data.error||'Local request failed');}
      selected=response.headers.get('x-agentbanter-trace')||'';session=response.headers.get('x-agentbanter-conversation')||session;linkConversation();if(selected)history.replaceState(null,'','/local?trace='+encodeURIComponent(selected));
      reader=response.body.getReader();while(true){const {done,value}=await reader.read();if(done)break;buffer+=decoder.decode(value,{stream:true});let end;while((end=buffer.indexOf('\n'))>=0){frame(buffer.slice(0,end));buffer=buffer.slice(end+1);}}buffer+=decoder.decode();if(buffer.trim())frame(buffer);$('local-prompt').value='';
    }catch(error){$('local-state').textContent=error.name==='AbortError'?'Stopped · partial recording retained':error.message;}finally{reader?.releaseLock();lock(false);await recordings();}
  };
  document.addEventListener('visibilitychange',()=>{if(!document.hidden)recordings();});
  models();linkConversation();recordings();setInterval(recordings,1800);
})();
