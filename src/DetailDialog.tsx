import { useEffect, useRef, type ReactNode } from 'react';
import { createPortal } from 'react-dom';

export function DetailDialog({label,onClose,children}:{label:string;onClose:()=>void;children:ReactNode}) {
  const dialog=useRef<HTMLDialogElement>(null);
  const backdropPress=useRef(false);
  useEffect(()=>{
    const element=dialog.current!;
    const previousOverflow=document.body.style.overflow;
    const opener=document.activeElement instanceof HTMLElement?document.activeElement:null;
    document.body.style.overflow='hidden';
    element.showModal();
    return()=>{
      element.close();
      document.body.style.overflow=previousOverflow;
      if(opener?.isConnected)opener.focus({preventScroll:true});
    };
  },[]);
  const outside=(x:number,y:number)=>{
    const rect=dialog.current!.getBoundingClientRect();
    return x<rect.left||x>rect.right||y<rect.top||y>rect.bottom;
  };
  return createPortal(<dialog ref={dialog} className="detail-dialog" aria-label={label}
    onKeyDown={event=>{
      if(event.key!=='Tab')return;
      const items=[...event.currentTarget.querySelectorAll<HTMLElement>('button:not([disabled]),a[href],input:not([disabled]),select:not([disabled]),textarea:not([disabled]),summary,[tabindex]:not([tabindex="-1"])')].filter(el=>el.getClientRects().length>0);
      const target=event.shiftKey?items.at(-1):items[0];
      if(!items.length){event.preventDefault();return;}
      if(event.shiftKey&&document.activeElement===items[0]||!event.shiftKey&&document.activeElement===items.at(-1)){event.preventDefault();target?.focus();}
    }}
    onCancel={event=>{event.preventDefault();onClose();}}
    onPointerDown={event=>{backdropPress.current=event.target===event.currentTarget&&outside(event.clientX,event.clientY);}}
    onClick={event=>{if(backdropPress.current&&event.target===event.currentTarget&&outside(event.clientX,event.clientY))onClose();backdropPress.current=false;}}
  >{children}</dialog>,document.body);
}
