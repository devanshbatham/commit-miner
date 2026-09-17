import {useEffect,useId,useLayoutEffect,useRef,useState,type CSSProperties,type KeyboardEvent} from 'react';

export interface SelectOption {value:string;label:string;detail?:string;group?:string}
export function Select({label,value,options,onChange}:{label:string;value:string;options:SelectOption[];onChange:(value:string)=>void}) {
  const id=useId(),root=useRef<HTMLDivElement>(null),trigger=useRef<HTMLButtonElement>(null),menu=useRef<HTMLDivElement>(null);
  const [open,setOpen]=useState(false),[active,setActive]=useState(0),[position,setPosition]=useState<CSSProperties>({});
  const selected=Math.max(0,options.findIndex(option=>option.value===value)),current=options[selected];
  function show(index=selected){setActive(index);setOpen(true);}
  function choose(index:number){const option=options[index];if(option)onChange(option.value);setOpen(false);trigger.current?.focus({preventScroll:true});}
  useLayoutEffect(()=>{
    if(!open)return;
    const rect=trigger.current!.getBoundingClientRect(),gap=7,pad=12,width=Math.min(Math.max(rect.width,320),window.innerWidth-pad*2);
    const below=window.innerHeight-rect.bottom-gap-pad,above=rect.top-gap-pad,up=below<220&&above>below;
    setPosition({left:Math.max(pad,Math.min(rect.right-width,window.innerWidth-width-pad)),width,maxHeight:Math.max(80,Math.min(360,up?above:below)),...(up?{bottom:window.innerHeight-rect.top+gap}:{top:rect.bottom+gap})});
  },[open]);
  useEffect(()=>{
    if(!open)return;
    const outside=(event:PointerEvent)=>{if(!root.current?.contains(event.target as Node))setOpen(false);};
    const resize=()=>setOpen(false);
    const scroll=(event:Event)=>{if(!menu.current?.contains(event.target as Node))setOpen(false);};
    document.addEventListener('pointerdown',outside);window.addEventListener('resize',resize);window.addEventListener('scroll',scroll,true);
    return()=>{document.removeEventListener('pointerdown',outside);window.removeEventListener('resize',resize);window.removeEventListener('scroll',scroll,true);};
  },[open]);
  useLayoutEffect(()=>{if(open)document.getElementById(`${id}-${active}`)?.scrollIntoView({block:'nearest'});},[active,open,position,id]);
  function keydown(event:KeyboardEvent<HTMLButtonElement>){
    if(event.key==='Escape'&&open){event.preventDefault();event.stopPropagation();setOpen(false);return;}
    if(event.key==='Tab'){setOpen(false);return;}
    if(event.key==='ArrowDown'||event.key==='ArrowUp'){
      event.preventDefault();const step=event.key==='ArrowDown'?1:-1;
      if(!open)show();else setActive(i=>(i+step+options.length)%options.length);return;
    }
    if(event.key==='Home'||event.key==='End'){event.preventDefault();show(event.key==='Home'?0:options.length-1);return;}
    if(event.key==='Enter'||event.key===' '){event.preventDefault();if(open)choose(active);else show();}
  }
  return <div className="select" ref={root}>
    <button ref={trigger} type="button" role="combobox" className="select-trigger" aria-label={label} aria-haspopup="listbox" aria-expanded={open} aria-controls={open?id:undefined} aria-activedescendant={open?`${id}-${active}`:undefined} onKeyDown={keydown} onClick={()=>open?setOpen(false):show()} disabled={!options.length}>
      <span className="select-value">{current?.label??label}</span>{current?.detail&&<span className="select-detail">{current.detail}</span>}
      <svg className="select-chevron" width="14" height="14" viewBox="0 0 16 16" fill="none" aria-hidden="true"><path d="m4 6 4 4 4-4" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round"/></svg>
    </button>
    {open&&<div ref={menu} id={id} role="listbox" aria-label={label} className="select-menu" style={position}>
      {options.map((option,index)=><div key={option.value} role="presentation">
        {option.group&&option.group!==options[index-1]?.group&&<div className="select-group" role="presentation">{option.group}</div>}
        <div id={`${id}-${index}`} role="option" aria-selected={option.value===value} className={`select-option ${index===active?'active':''}`} onPointerMove={()=>setActive(index)} onPointerDown={event=>event.preventDefault()} onClick={()=>choose(index)}>
          <span className="select-check" aria-hidden="true">{option.value===value?'✓':''}</span><span className="select-option-label">{option.label}</span>{option.detail&&<span className="select-detail">{option.detail}</span>}
        </div>
      </div>)}
    </div>}
  </div>;
}
