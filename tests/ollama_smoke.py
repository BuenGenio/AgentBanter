#!/usr/bin/env python3
"""Exercise recording/chat against a deterministic local Ollama HTTP fixture."""
import http.server
import json
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import threading
import time
import urllib.error
import urllib.parse
import urllib.request

binary=Path(sys.argv[1]).resolve()
requests=[]
thinking='Check 17 times 23: 340 plus 51 is 391. 🌈'
class Ollama(http.server.BaseHTTPRequestHandler):
    def log_message(self,*args): pass
    def respond(self,value,status=200,kind='application/json'):
        data=value if isinstance(value,bytes) else json.dumps(value,ensure_ascii=False).encode()
        self.send_response(status);self.send_header('Content-Type',kind);self.send_header('Content-Length',str(len(data)));self.end_headers();self.wfile.write(data)
    def do_GET(self):
        if self.path=='/api/tags':self.respond({'models':[{'name':'test-model','size':4_000_000_000}]})
        elif self.path=='/v1/models':self.respond({'object':'list','data':[{'id':'test-model'}]})
        else:self.respond({'version':'fixture'})
    def do_POST(self):
        body=json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        requests.append((self.path,body))
        if self.path=='/api/show':return self.respond({'capabilities':['completion','thinking'],'thinking':{'values':[False,True],'default':True}})
        if body['model']=='missing':return self.respond({'error':'model not found'},404)
        if body.get('format')=='json':
            analysis={'summary':'The trace decomposes a product.','steps':[{'claim':'It adds 340 and 51.','evidence_quote':'340 plus 51 is 391.'},{'claim':'Unsupported claim','evidence_quote':'An invented quote'}],'assumptions':[],'alternatives':[],'issues':[],'limits':'This interpretation describes emitted text.'}
            return self.respond({'message':{'role':'assistant','content':json.dumps(analysis)},'done':True,'eval_count':15})
        if self.path=='/v1/chat/completions':
            chunks=[{'choices':[{'delta':{'reasoning_content':thinking}}]},{'choices':[{'delta':{'content':'391.'},'finish_reason':'stop'}],'usage':{'completion_tokens':17}}]
            data=(''.join('data: '+json.dumps(v,ensure_ascii=False)+'\n\n' for v in chunks)+'data: [DONE]\n\n').encode()
            return self.respond(data,kind='text/event-stream')
        message={'role':'assistant','thinking':thinking,'content':'391.'}
        if body.get('stream') is False:
            result={'message':message,'done':True,'eval_count':17,'eval_duration':1_000_000_000}
            if self.path=='/api/generate':result.update(thinking=thinking,response='391.');result.pop('message')
            return self.respond(result)
        chunks=[{'message':{'thinking':thinking},'done':False},{'message':{'content':'391.'},'done':False},{'done':True,'eval_count':17,'eval_duration':1_000_000_000}]
        data=''.join(json.dumps(v,ensure_ascii=False)+'\n' for v in chunks).encode()
        self.respond(data,kind='application/x-ndjson')

fixture=http.server.ThreadingHTTPServer(('127.0.0.1',0),Ollama)
if len(sys.argv)>1 and sys.argv[1]=='--fixture':
    print(f'Fixture listening on {fixture.server_port}',flush=True)
    fixture.serve_forever()
    raise SystemExit(0)
threading.Thread(target=fixture.serve_forever,daemon=True).start()
with tempfile.TemporaryDirectory(prefix='agentbanter-ollama-smoke-') as temp:
    root=Path(temp)
    def port():
        with socket.socket() as sock:sock.bind(('127.0.0.1',0));return sock.getsockname()[1]
    def start(enabled):
        selected=port();command=[str(binary),'serve','--port',str(selected),'--bind','127.0.0.1','--sources','ollama','--ollama-dir',str(root/'recordings'),'--ollama-url',f'http://127.0.0.1:{fixture.server_port}']
        if enabled:command.append('--allow-ollama')
        process=subprocess.Popen(command,cwd=root,stdout=subprocess.DEVNULL,stderr=subprocess.PIPE)
        base=f'http://127.0.0.1:{selected}'
        for attempt in range(60):
            try:urllib.request.urlopen(base+'/healthz',timeout=1).close();return process,base
            except urllib.error.URLError:
                if process.poll() is not None:raise RuntimeError(process.stderr.read().decode())
                time.sleep(.05)
        raise RuntimeError('Server did not start')
    server,base=start(True)
    def get(path):
        with urllib.request.urlopen(base+path,timeout=5) as response:return response.read()
    def post(path,body,headers=None):
        request=urllib.request.Request(base+path,data=json.dumps(body).encode(),headers={'Content-Type':'application/json',**(headers or {})})
        with urllib.request.urlopen(request,timeout=10) as response:return response.read(),dict(response.headers)
    def trace(id):return json.loads(get('/local/traces/'+id))
    try:
        assert b'Local models' in get('/local')
        assert json.loads(get('/local/models'))['models'][0]['name']=='test-model'
        assert len(get('/static/local.js'))>1000
        body={'model':'test-model','messages':[{'role':'user','content':'17 x 23?'}],'think':True,'stream':True,'options':{'temperature':.2}}
        payload,headers=post('/ollama/api/chat',body,{'X-AgentBanter-Conversation':'external'})
        id=headers['x-agentbanter-trace']
        assert payload==''.join(json.dumps(v,ensure_ascii=False)+'\n' for v in [{'message':{'thinking':thinking},'done':False},{'message':{'content':'391.'},'done':False},{'done':True,'eval_count':17,'eval_duration':1_000_000_000}]).encode()
        recorded=trace(id);assert recorded['thinking']==thinking and recorded['answer']=='391.' and recorded['status']=='complete'
        assert requests[-1]==('/api/chat',body)
        raw=next((root/'recordings').glob('external.jsonl')).read_text();assert 'ollama_request' in raw and 'ollama_chunk' in raw
        assert b'Check 17 times 23' in get('/s/ollama--external')
        result=json.loads(get('/search?'+urllib.parse.urlencode({'q':'340 plus 51','scope':'ollama--external','kind':'reasoning'})))
        assert result['count']==1
        analysis,_=post('/local/traces/'+id+'/analysis',{})
        analysis=json.loads(analysis);assert analysis['analysis']['steps'][0]['quote_verified'] is True and analysis['analysis']['steps'][1]['quote_verified'] is False
        child=trace(analysis['analysis_trace_id']);assert child['origin']=='analysis' and child['session']!='ollama--external'
        members=json.loads(get('/s/ollama--external/team'))['members'];assert len(members)==2 and all(not m['sendable'] for m in members)
        child_page=get('/s/'+child['session']);assert b'Analysis of recorded thinking' in child_page
        before=len(requests);cached,_=post('/local/traces/'+id+'/analysis',{});assert json.loads(cached)==analysis and len(requests)==before
        payload,headers=post('/ollama/v1/chat/completions',body)
        assert payload.endswith(b'data: [DONE]\n\n') and trace(headers['x-agentbanter-trace'])['thinking']==thinking
        nonstream={**body,'stream':False}
        payload,headers=post('/ollama/api/chat',nonstream)
        assert json.loads(payload)['done'] is True and trace(headers['x-agentbanter-trace'])['thinking']==thinking
        payload,headers=post('/ollama/api/generate',{'model':'test-model','prompt':'17 x 23?','stream':False})
        assert trace(headers['x-agentbanter-trace'])['answer']=='391.'
        payload,headers=post('/local/chat',{'model':'test-model','text':'Another question','session':'ollama--external','think':True})
        assert requests[-1][1]['messages'][0]['content']=='17 x 23?' and requests[-1][1]['messages'][-1]['content']=='Another question'
        for path,extra,status in [('/ollama/api/chat',{'Origin':'https://other.test'},403),('/ollama/api/pull',{},404),('/local/chat',{'Origin':'https://other.test'},403)]:
            before=len(requests)
            try:post(path,{'model':'test-model','text':'blocked'} if path=='/local/chat' else body,extra);raise AssertionError('Unexpected acceptance')
            except urllib.error.HTTPError as error:assert error.code==status
            assert len(requests)==before
        try:post('/ollama/api/chat',{'model':'missing','messages':[]});raise AssertionError('Missing model accepted')
        except urllib.error.HTTPError as error:assert error.code==404 and json.loads(error.read())['error']=='model not found'
        print('Ollama smoke passed: byte-preserving native/SSE proxy, direct chat/history, persistent traces, analyst child/verified quotes, search, gate and origin checks.')
    finally:server.terminate();server.wait(timeout=5)
    restarted,base=start(True)
    try:
        saved=trace(id)
        assert saved['thinking']==thinking and saved['analysis']==analysis
        assert b'Check 17 times 23' in get('/s/ollama--external')
        print('Recordings and generated analysis survive a server restart.')
    finally:restarted.terminate();restarted.wait(timeout=5)
    disabled,base=start(False)
    try:
        before=len(requests)
        try:post('/ollama/api/chat',body);raise AssertionError('Disabled gateway accepted inference')
        except urllib.error.HTTPError as error:assert error.code==403
        assert len(requests)==before
        print('Disabled gateway refused inference.')
    finally:disabled.terminate();disabled.wait(timeout=5)
fixture.shutdown()
