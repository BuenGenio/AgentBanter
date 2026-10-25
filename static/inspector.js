// All record text enters via textContent; Highlight.js only receives escaped code.
(() => {
  const el = (tag, cls, text) => { const node = document.createElement(tag); if (cls) node.className = cls; if (text !== undefined) node.textContent = text; return node; };
  const decode = window.AgentDecoder.decode;
  function tree(value, key = '$', depth = 0, path = '$') {
    const decoded = decode(value);
    const encoded = decoded !== value && typeof value === 'string';
    value = decoded;
    const isObject = value !== null && typeof value === 'object';
    const type = value === null ? 'null' : Array.isArray(value) ? 'array' : typeof value;
    const label = el('span', 'tree-key', key);
    label.title = path;
    const typeLabel = el('span', 'tree-type', (encoded ? 'JSON string → ' : '') + type);
    if (typeof value === 'string' && (value.includes('\n') || value.length > 120)) {
      const details=el('details','tree-node tree-text');
      const summary=el('summary');
      summary.append(label,typeLabel,el('span','tree-count',`${value.length} chars`),el('span','tree-preview',value.split('\n')[0].slice(0,75)));
      details.append(summary);
      let rendered=false;
      details.addEventListener('toggle',()=>{if(details.open&&!rendered){rendered=true;details.append(code(value,window.AgentDecoder.language(key,value)));}});
      return details;
    }
    if (!isObject || depth >= 24) {
      const leaf = el('div', 'tree-leaf');
      const text = typeof value === 'string' ? JSON.stringify(value) : JSON.stringify(value);
      leaf.append(label, el('span','tree-colon',':'), typeLabel, el('span', 'tree-value tree-' + type, text));
      return leaf;
    }
    const entries = Object.entries(value);
    const details = el('details','tree-node');
    const summary = el('summary');
    summary.append(label, typeLabel, el('span','tree-count', `${Array.isArray(value) ? '[' : '{'} ${entries.length} ${entries.length === 1 ? 'item' : 'items'} ${Array.isArray(value) ? ']' : '}'}`));
    details.append(summary);
    let rendered = false;
    function populate() {
      if (rendered) return;
      rendered = true;
      const children = el('div','tree-children');
      let offset = 0;
      function page() {
        const end = Math.min(offset + 100, entries.length);
        for (; offset < end; offset++) {
          const [k,v] = entries[offset]; children.append(tree(v,k,depth+1, path + (Array.isArray(value) ? `[${k}]` : '.'+k)));
        }
        if (offset < entries.length) {
          const more = el('button','tree-more',`Show next ${Math.min(100,entries.length-offset)} items`);
          more.type='button'; more.onclick=()=>{more.remove();page();}; children.append(more);
        }
      }
      page(); details.append(children);
    }
    details.addEventListener('toggle',()=>{if(details.open)populate();});
    if (depth === 0) { details.open = true; populate(); }
    return details;
  }
  function code(text, language) {
    const pre = el('pre','inspector-code'); const content = el('code',language ? 'language-'+language : '',text); pre.append(content);
    if (window.hljs && text.length < 100000) {
      if (!language) content.classList.add('language-plaintext');
      hljs.highlightElement(content);
    }
    return pre;
  }
  window.renderInspector = (host, raw, readable) => {
    host.replaceChildren(); host.dataset.enhanced = '1';
    let value = decode(raw); const parsed = typeof value === 'object' && value !== null;
    const bar = el('div','inspector-tabs'); const content = el('div','inspector-body');
    const tabs = [];
    function choose(button, build) {
      tabs.forEach(b=>{b.classList.toggle('active',b===button); b.setAttribute('aria-pressed',String(b===button));});
      content.replaceChildren(build());
    }
    function tab(name, build) { const button=el('button','',name);button.type='button';button.onclick=()=>choose(button,build);tabs.push(button);bar.append(button);return button; }
    if (readable !== undefined) tab('Output',()=>code(readable, typeof decode(readable)==='object' ? 'json' : 'plaintext'));
    const treeTab = tab('Object tree',()=>tree(value));
    tab('JSON',()=>code(JSON.stringify(value,null,2),'json'));
    tab('Original',()=>code(typeof raw==='string' ? raw : JSON.stringify(raw,null,2),parsed?'json':'plaintext'));
    const copy = el('button','copy-record','Copy');copy.type='button';copy.onclick=async()=>{try{await navigator.clipboard.writeText(content.textContent);copy.textContent='Copied';}catch{copy.textContent='Copy unavailable';}};bar.append(copy);
    host.append(bar,content);(readable !== undefined ? tabs[0] : treeTab).click();
  };
  window.enhanceCode = root => {
    root.querySelectorAll('.input-inspector:not([data-enhanced])').forEach(host=>{
      host.dataset.enhanced='1';
      const original=host.querySelector('pre code');if(!original)return;
      const snippets=window.AgentDecoder.snippets(original.textContent);
      if(!snippets.length)return;
      const raw=el('details','invocation-original');raw.append(el('summary','','Original invocation'));
      raw.append(host.querySelector('pre'));host.replaceChildren();
      for(const [index,snippet] of snippets.entries()) {
        const section=el('details','decoded-snippet');section.open=index===0;
        section.append(el('summary','',`${snippet.label} · ${snippet.language} · decoded`));
        section.append(code(snippet.text,snippet.language));host.append(section);
      }
      host.append(raw);
    });
    root.querySelectorAll('.output-inspector:not([data-enhanced])').forEach(host=>{
      const data = host.querySelector('.inspector-data'); if (!data) return;
      try { window.renderInspector(host,JSON.parse(data.textContent),host.querySelector('.rdetail')?.textContent || ''); } catch {}
    });
    root.querySelectorAll('pre code:not([data-highlighted])').forEach(node=>{
      if (node.textContent.length < 100000 && window.hljs) {
        if (!node.className) node.className='language-json';
        hljs.highlightElement(node);
      }
    });
  };
  if(window.hljs)hljs.configure({languages:['javascript','json','bash','python','rust','css','xml','diff','plaintext']});
  window.enhanceCode(document);
})();
