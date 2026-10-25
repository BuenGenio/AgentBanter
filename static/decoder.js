(function(root, factory) {
  if (typeof module === 'object' && module.exports) module.exports = factory(require('./vendor/acorn.js'));
  else root.AgentDecoder = factory(root.acorn);
})(typeof globalThis !== 'undefined' ? globalThis : this, function(acorn) {
  function decode(value) {
    for(let i=0;i<12 && typeof value==='string' && value.length<1000000;i++) {
      const text=value.trim();
      if (!/^[\[\{"'`]/.test(text)) break;
      let next;
      try { next=JSON.parse(text); }
      catch {
        try {
          const node=acorn.parseExpressionAt(text,0,{ecmaVersion:'latest'});
          if(node.end!==text.length)break;
          if(node.type==='Literal'&&typeof node.value==='string')next=node.value;
          else if(node.type==='TemplateLiteral'&&node.expressions.length===0)next=node.quasis[0].value.cooked;
          else break;
        } catch { break; }
      }
      if(next===value)break;value=next;
    }
    return value;
  }
  function language(key, text) {
    if (/^(cmd|command|shell)$/i.test(key))return 'bash';
    if (/patch|diff/i.test(key)||text.startsWith('*** Begin Patch'))return 'diff';
    if (/\.css$/.test(key)||/^\s*(?::root|@media|\.[\w-]+\s*\{)/.test(text))return 'css';
    if (/\.py$/.test(key)||/^\s*(?:from \w+ import|import \w+|def \w+\(|print\()/.test(text))return 'python';
    if (/\.js$/.test(key)||/\b(?:const |let |function |await |=>)/.test(text))return 'javascript';
    if (/\.html$/.test(key)||/^\s*<(?:!doctype|html|div|section)/i.test(text))return 'xml';
    try {const data=JSON.parse(text);if(typeof data==='object'&&data!==null)return 'json';}catch{}
    return 'plaintext';
  }
  function heredocs(shell) {
    const lines=shell.split('\n'),out=[];
    for(let i=0;i<lines.length;i++) {
      const match=lines[i].match(/<<(-?)\s*['"]?([A-Za-z_][\w]*)['"]?/);
      if(!match)continue;
      const header=lines[i],body=[];let end=i+1;
      while(end<lines.length&&(match[1]?lines[end].replace(/^\t+/,''):lines[end])!==match[2])body.push(lines[end++]);
      if(end===lines.length)continue;
      const file=header.match(/>{1,2}\s*([^\s]+)/)?.[1];
      const lang=/\bpython\d*\b/.test(header)?'python':/\bnode\b/.test(header)?'javascript':language(file||'',body.join('\n'));
      out.push({label:file||`${lang} heredoc`,text:body.join('\n'),language:lang});i=end;
    }
    return out;
  }
  function snippets(source) {
    if(source.length>1000000)return [];
    let ast;try{ast=acorn.parse(source,{ecmaVersion:'latest',allowAwaitOutsideFunction:true});}catch{return [];}
    const out=[],seen=new Set();let visited=0;
    function visit(node, depth = 0) {
      if(!node||typeof node!=='object'||depth>40||++visited>20000||out.length>=40)return;
      if(node.type==='Property'&&!node.computed) {
        const key=node.key.name||node.key.value;
        const value=node.value.type==='Literal'?node.value.value:node.value.type==='TemplateLiteral'&&node.value.expressions.length===0?node.value.quasis[0].value.cooked:null;
        if(typeof value==='string'&&(/^(cmd|command|code|script|input|patch|content|text)$/.test(key))&&(value.includes('\n')||/^(cmd|command|code|script|patch)$/.test(key))&&!seen.has(value)) {
          seen.add(value);const decoded=decode(value);const text=typeof decoded==='string'?decoded:JSON.stringify(decoded,null,2);
          out.push({label:key,text,language:language(key,text)});
          if(/^(cmd|command)$/.test(key))out.push(...heredocs(text));
        }
      }
      for(const [key,value]of Object.entries(node)) {
        if(key==='start'||key==='end')continue;
        if(Array.isArray(value))value.forEach(child=>visit(child,depth+1));else if(value&&typeof value==='object')visit(value,depth+1);
      }
    }
    visit(ast);return out;
  }
  return {decode,language,snippets,heredocs};
});
