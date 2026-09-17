import {useMemo,useState} from 'react';
import type {CommitResult,DiffLine} from '../shared/types';
import {categoryById} from '../shared/taxonomy';
import {DetailDialog} from './DetailDialog';
import {Select} from './Select';
export function splitLines(lines:DiffLine[]){
  const rows:{left?:DiffLine;right?:DiffLine}[]=[];
  for(let i=0;i<lines.length;){
    if(lines[i].kind==='removed'||lines[i].kind==='added'){
      const removed:DiffLine[]=[],added:DiffLine[]=[];
      while(i<lines.length&&(lines[i].kind==='removed'||lines[i].kind==='added')){const line=lines[i++];(line.kind==='removed'?removed:added).push(line);}
      for(let j=0;j<Math.max(removed.length,added.length);j++)rows.push({left:removed[j],right:added[j]});
    }else{rows.push({left:lines[i],right:lines[i]});i++;}
  }
  return rows;
}
export function CommitDialog({result,source,onClose}:{result:CommitResult;source:string;onClose:()=>void}){
  const full=result,error='';
  const [file,setFile]=useState(result.evidence[0]?.evidence.path??''),[part,setPart]=useState(0),[layout,setLayout]=useState<'split'|'unified'>(()=>window.innerWidth<750?'unified':'split');
  const commit=result.commit,files=useMemo(()=>[...new Set(full?.evidence.map(e=>e.evidence.path)??[])],[full]);
  const sections=full?.evidence.filter(e=>e.evidence.path===file)??[],section=sections[Math.min(part,Math.max(0,sections.length-1))];
  const rows=section?splitLines(section.evidence.lines):[];
  let commitURL:string|undefined;try{const url=new URL(source);if(url.protocol==='https:'&&url.hostname==='github.com'&&!url.username&&!url.password){let base=url.pathname;if(base.endsWith('/'))base=base.slice(0,-1);if(base.endsWith('.git'))base=base.slice(0,-4);commitURL=`https://github.com${base}/commit/${commit.sha}`;}}catch{/* Local repositories have no remote link. */}
  return <DetailDialog label="Commit diff" onClose={onClose}>
    <header className="dialog-header"><div><div className="eyebrow"><>{commitURL?<a href={commitURL} target="_blank" rel="noreferrer">{commit.sha.slice(0,10)} ↗</a>:<code>{commit.sha.slice(0,10)}</code>}</><span>{commit.author} · {commit.date?new Date(commit.committedAt||commit.date).toLocaleDateString():'—'}</span></div><h2>{commit.message.split('\n')[0]}</h2></div><button className="icon" aria-label="Close commit" onClick={onClose}>×</button></header>
    <div className="dialog-summary"><div className="badges">{result.categories.map(c=>{const category=categoryById[c.id]??{family:'change',label:c.id,cwe:undefined};return <span className={`badge ${category.family}`} key={c.id}>{category.label}{category.cwe?` · CWE-${category.cwe}`:''}<small>{Math.round(c.probability*100)}%</small></span>;})}{!result.categories.length&&<span className="muted">{result.status==='failed'?'Failed':result.status==='pending'?'Not analyzed':result.reviewCoverage==='metadata'?'Metadata review':'Unclassified'}</span>}</div><details open><summary>Commit details</summary><pre className="message">{commit.message}</pre><div className="scores"><span>Bug fix <b>{result.probabilities.bug_fix===undefined?'—':Math.round(result.probabilities.bug_fix*100)+'%'}</b></span><span>Security fix <b>{result.probabilities.security_fix===undefined?'—':Math.round(result.probabilities.security_fix*100)+'%'}</b></span><span>Needs context <b>{result.probabilities.insufficient_context===undefined?'—':Math.round(result.probabilities.insufficient_context*100)+'%'}</b></span></div>{(full??result).warnings.filter(w=>!w.startsWith('Final review used ')).map((w,i)=><p className="coverage" key={i}>{w}</p>)}</details></div>
    {error?<p className="notice" role="alert">{error}</p>:!full?<div className="loading">Loading diff…</div>:<div className="diff-workspace"><nav aria-label="Changed files" className="diff-files"><span className="eyebrow">{files.length} changed files</span>{files.map(path=><button key={path} aria-pressed={file===path} onClick={()=>{setFile(path);setPart(0);}}><span>{path}</span><small>{full.evidence.filter(e=>e.evidence.path===path).length}</small></button>)}</nav><section className="diff-body"><div className="diff-toolbar"><div className="mobile-files"><Select label="Changed file" value={file} onChange={value=>{setFile(value);setPart(0);}} options={files.map(path=>({value:path,label:path}))}/></div><span className="diff-path">{file}</span><div className="segmented" aria-label="Diff layout"><button aria-pressed={layout==='split'} onClick={()=>setLayout('split')}>Split</button><button aria-pressed={layout==='unified'} onClick={()=>setLayout('unified')}>Unified</button></div></div>
    {sections.length>1&&<div className="section-nav"><button disabled={part===0} onClick={()=>setPart(part-1)}>← Previous</button><span>Section {part+1} of {sections.length}</span><button disabled={part===sections.length-1} onClick={()=>setPart(part+1)}>Next →</button></div>}
    {section?<div className={`code-window ${layout}`} tabIndex={0} aria-label="Diff evidence"><div className="code-labels">{layout==='split'?<><span>Before</span><span>After</span></>:<span>Before → After</span>}</div><div className="hunk-header">{section.evidence.header}</div>{layout==='unified'?section.evidence.lines.map((line,i)=><div key={i} className={`unified-line ${line.kind}`}><span className="line-number">{line.oldLine??''}</span><span className="line-number">{line.newLine??''}</span><span className="diff-sign">{line.kind==='added'?'+':line.kind==='removed'?'−':' '}</span><code>{line.text||' '}</code></div>):rows.map((row,i)=><div className="split-row" key={i}>{(['left','right'] as const).map(side=>{const line=row[side];return <div key={side} className={`split-cell ${line?.kind??'blank'}`}><span className="line-number">{(side==='left'?line?.oldLine:line?.newLine)??''}</span><span className="diff-sign">{line?.kind==='added'?'+':line?.kind==='removed'?'−':' '}</span><code>{line?.text||' '}</code></div>;})}</div>)}</div>:<p className="empty-small">No textual diff available.</p>}
    </section></div>}
  </DetailDialog>;
}
