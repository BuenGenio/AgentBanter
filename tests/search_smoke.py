#!/usr/bin/env python3
"""Check search scopes and exact anchors against an installed, embedded-assets server."""
import json
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request

binary = Path(sys.argv[1]).resolve()
with tempfile.TemporaryDirectory(prefix='agentbanter-search-') as temp:
    root = Path(temp)
    claude = root / 'claude' / 'project'
    child = claude / 'root' / 'subagents'
    codex = root / 'codex'
    empty = root / 'empty'
    for folder in (child, codex, empty):
        folder.mkdir(parents=True)
    def write(path, records):
        path.write_text(''.join(json.dumps(record) + '\n' for record in records))
    def prompt(uuid, text):
        return {'type':'user','uuid':uuid,'cwd':'/project','message':{'content':text}}
    write(claude / 'root.jsonl', [
        prompt('root-prompt','rainbow needle'),
        {'type':'assistant','uuid':'root-tool','message':{'content':[{'type':'tool_use','id':'a','name':'Bash','input':{'command':'echo needle'}}]}},
        {'type':'user','message':{'content':[{'type':'tool_result','tool_use_id':'a','content':'needle output'}]}}
    ])
    write(child / 'agent-child.jsonl', [prompt('child-prompt','child needle')])
    write(claude / 'neighbor.jsonl', [prompt('neighbor-prompt','neighbor needle')])
    write(codex / 'rollout.jsonl', [
        {'type':'session_meta','payload':{'id':'codex-root','cwd':'/different'}},
        {'type':'response_item','payload':{'type':'message','id':'codex-prompt','role':'user','content':[{'type':'input_text','text':'codex needle'}]}}
    ])
    with socket.socket() as sock:
        sock.bind(('127.0.0.1',0))
        port = sock.getsockname()[1]
    base = f'http://127.0.0.1:{port}'
    server = subprocess.Popen([str(binary),'serve','--bind','127.0.0.1','--port',str(port),'--sources','claude,codex','--projects-dir',str(root/'claude'),'--codex-dir',str(codex),'--chatgpt-dir',str(empty)],cwd=root,stdout=subprocess.DEVNULL,stderr=subprocess.PIPE)
    def get(path):
        with urllib.request.urlopen(base+path,timeout=5) as response:
            return response.read().decode()
    def search(**params):
        return json.loads(get('/search?'+urllib.parse.urlencode(params)))
    try:
        for attempt in range(50):
            try:
                get('/healthz')
                break
            except urllib.error.URLError:
                if server.poll() is not None:
                    raise RuntimeError(server.stderr.read().decode())
                time.sleep(.05)
        else:
            raise RuntimeError('Server did not start')
        assert search(q='needle')['count'] == 5
        scoped = search(q='needle',scope='root')
        assert scoped['count'] == 3
        assert set(scoped['sessions']) == {'root','claude-agent--root--agent-child'}
        assert search(q='needle',scope='root',kind='command')['anchors'] == ['tool-a']
        only_child = search(q='needle',scope='root',agent='claude-agent--root--agent-child')
        assert only_child['count'] == 1 and only_child['anchors'] == []
        assert search(q='NEEDLE',scope='root',case='true')['count'] == 0
        assert search(q='rainbow.*needle',scope='root',regex='true')['count'] == 1
        try:
            search(q='[',regex='true')
            raise AssertionError('Invalid regex accepted')
        except urllib.error.HTTPError as error:
            assert error.code == 400 and 'Invalid search pattern' in error.read().decode()
        for hit in search(q='needle')['hits']:
            page = get('/s/'+urllib.parse.quote(hit['session']))
            assert f'id="{hit["anchor"]}"' in page, hit
            assert 'id="global-query"' in page and 'id="task-query"' in page
            assert 'id="session-search"' not in page
        for asset in ('search.js','lights.js','structure.js'):
            assert len(get('/static/'+asset)) > 500
        print('Search smoke passed: providers, family and agent scopes, commands, regex, exact rendered anchors, and embedded assets.')
    finally:
        server.terminate()
        server.wait(timeout=5)
