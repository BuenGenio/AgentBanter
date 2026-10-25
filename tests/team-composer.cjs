const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
class Element {
  constructor(tag='div'){this.tag=tag;this.children=[];this.value='';this.dataset={};this.listeners={};}
  append(...items){this.children.push(...items);}
  replaceChildren(...items){this.children=items;}
  get options(){return this.children;}
  addEventListener(name,fn){this.listeners[name]=fn;}
  querySelector(){return button;}
}
const ids=Object.fromEntries(['team-panel','composer','composer-status','team-note','team-members','delivery-mode','recipient-link'].map(id=>[id,new Element()]));
const select=new Element('select'), box=new Element('textarea'), button=new Element('button');
ids['team-panel'].dataset.session='root';
ids.composer.elements={recipient:select,text:box};
const members=[
 {id:'root',name:'Lead',provider:'Codex',sendable:true},
 {id:'child',name:'Reviewer',provider:'Codex',sendable:true,parent_name:'Lead'},
 {id:'claude-child',name:'Reader',provider:'Claude',sendable:false,parent_name:'Lead'}
];
const sent=[], timers=[];
const code=fs.readFileSync('static/app.js','utf8').split('// Team membership')[1].split('// Process references')[0];
vm.runInNewContext('// Team membership'+code,{
 document:{hidden:false,getElementById:id=>ids[id],createElement:tag=>new Element(tag)},
 setTimeout:fn=>timers.push(fn),encodeURIComponent,
 fetch:async (url,options)=>{
  if(url.endsWith('/team'))return {ok:true,json:async()=>({members,note:'Recorded team'})};
  if(options){sent.push({url,body:JSON.parse(options.body)});return {ok:!url.includes('child'),json:async()=>url.includes('child')?{error:'Unavailable'}:{state:'queued',session:'root'}};}
  throw Error('unexpected polling');
 }
});
(async()=>{
 await new Promise(setImmediate);
 assert.equal(select.options.length,4);
 assert.equal(select.options.find(o=>o.value==='claude-child').disabled,true);
 select.value='@team';select.listeners.change();box.value='Hello everyone';
 await ids.composer.listeners.submit({preventDefault(){}});
 await new Promise(setImmediate);
 assert.deepEqual(sent.map(s=>s.url),['/s/root/send','/s/child/send']);
 assert(sent.every(s=>s.body.text==='Hello everyone'));
 assert.equal(box.value,'Hello everyone','Partial failure retains the draft');
 assert.equal(button.disabled,false);
 assert(ids['composer-status'].children.some(c=>c.textContent?.includes('Draft retained')));
 console.log('Team composer: explicit recipients, child exclusion, partial-failure receipts passed.');
})().catch(e=>{console.error(e);process.exitCode=1;});
