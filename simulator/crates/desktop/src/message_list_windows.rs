//! Native inspector UI on its own host thread. Only owned rows and source
//! input ids cross the channel; no game process or original dialog is loaded.
use anyhow::{Result, bail};
use cs2_runtime::message_list::MessageListRow;

use crate::ScriptTextEncoding;

pub(crate) enum Event { Selected(u32), Hidden, Failure(String) }

#[cfg(not(windows))]
pub(crate) struct Viewer;

#[cfg(not(windows))]
impl Viewer {
    pub(crate) fn ready(&mut self) -> Result<bool> { bail!("native message-list window is Windows-only") }
    pub(crate) fn visible(&mut self, _: bool) -> Result<()> { bail!("use the portable message-list view") }
    pub(crate) fn events(&mut self) -> Result<Vec<Event>> { bail!("native message-list window is Windows-only") }
    pub(crate) fn replace(&mut self, _: &[MessageListRow], _: ScriptTextEncoding) -> Result<()> {
        bail!("use the portable message-list view")
    }
    pub(crate) fn close(&mut self) -> Result<bool> { bail!("native message-list window is Windows-only") }
}

#[cfg(windows)]
pub(crate) use win::Viewer;

#[cfg(windows)]
mod win {
    use super::*;
    use anyhow::Context;
    use std::cell::RefCell;
    use std::ffi::c_void;
    use std::ptr::null;
    use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
    use std::thread::{self, JoinHandle};
    use std::time::Duration;

    enum Command { Rows(Vec<MessageListRow>, ScriptTextEncoding), Visible(bool), Close }

    pub(crate) struct Viewer {
        commands: Sender<Command>,
        notices: Receiver<Event>,
        ready: Option<Receiver<Result<()>>>,
        worker: Option<JoinHandle<()>>,
        closing: bool,
    }

    impl Viewer {
        pub(crate) fn open() -> Result<Self> {
            Self::open_with_title("CST message list")
        }

        pub(crate) fn open_with_title(title: &str) -> Result<Self> {
            let (commands, requests) = mpsc::channel();
            let (events, notices) = mpsc::channel();
            let (started, ready) = mpsc::channel();
            let title = title.to_owned();
            let worker = thread::Builder::new().name("cs2-message-list".into()).spawn(move || {
                if let Err(cause) = run(requests, &events, &started, &title) {
                    let message = format!("{cause:#}");
                    let _ = started.send(Err(anyhow::anyhow!(message.clone())));
                    let _ = events.send(Event::Failure(message));
                }
            }).context("start message-list UI thread")?;
            Ok(Self { commands, notices, ready: Some(ready), worker: Some(worker), closing: false })
        }

        pub(crate) fn ready(&mut self) -> Result<bool> {
            let Some(ready) = &self.ready else { return Ok(true); };
            match ready.try_recv() {
                Ok(result) => { result?; self.ready = None; Ok(true) }
                Err(TryRecvError::Empty) => Ok(false),
                Err(TryRecvError::Disconnected) => bail!("message-list window startup disconnected"),
            }
        }

        pub(crate) fn visible(&mut self, value: bool) -> Result<()> {
            self.commands.send(Command::Visible(value)).context("show/hide message-list window")
        }

        pub(crate) fn events(&mut self) -> Result<Vec<Event>> {
            let mut events = Vec::new();
            while let Ok(event) = self.notices.try_recv() { events.push(event); }
            if !self.closing && self.worker.as_ref().is_some_and(JoinHandle::is_finished)
                && events.is_empty()
            {
                bail!("message-list UI thread stopped");
            }
            Ok(events)
        }

        pub(crate) fn replace(&mut self, rows: &[MessageListRow], encoding: ScriptTextEncoding) -> Result<()> {
            self.commands.send(Command::Rows(rows.to_vec(), encoding))
                .context("submit actual message-list rows")
        }

        pub(crate) fn close(&mut self) -> Result<bool> {
            if !self.closing {
                // A worker which already failed still has to be joined.
                let _ = self.commands.send(Command::Close);
                self.closing = true;
            }
            if self.worker.as_ref().is_some_and(|worker| !worker.is_finished()) { return Ok(false); }
            if let Some(worker) = self.worker.take() {
                worker.join().map_err(|_| anyhow::anyhow!("message-list UI thread panicked"))?;
            }
            Ok(true)
        }
    }

    // Raw declarations avoid widening the workspace windows-sys feature set.
    #[repr(C)]
    struct WindowClass {
        style: u32, procedure: Option<unsafe extern "system" fn(isize,u32,usize,isize)->isize>,
        class_extra: i32, window_extra: i32, instance: isize, icon: isize, cursor: isize,
        background: isize, menu: *const u16, class: *const u16,
    }
    #[repr(C)]
    struct Message { window: isize, message: u32, first: usize, second: isize,
        time: u32, point: [i32; 2], private: u32 }
    #[repr(C)]
    struct Rectangle { left: i32, top: i32, right: i32, bottom: i32 }

    #[link(name="kernel32")]
    unsafe extern "system" { fn GetModuleHandleW(name: *const u16) -> isize; fn GetLastError() -> u32; }
    #[link(name="user32")]
    unsafe extern "system" {
        fn RegisterClassW(class: *const WindowClass) -> u16;
        fn CreateWindowExW(extra:u32,class:*const u16,title:*const u16,style:u32,
            x:i32,y:i32,width:i32,height:i32,parent:isize,menu:isize,instance:isize,data:*const c_void)->isize;
        fn DefWindowProcW(window:isize,message:u32,first:usize,second:isize)->isize;
        fn DestroyWindow(window:isize)->i32;
        fn ShowWindow(window:isize,show:i32)->i32;
        fn SendMessageW(window:isize,message:u32,first:usize,second:isize)->isize;
        fn PeekMessageW(message:*mut Message,window:isize,minimum:u32,maximum:u32,remove:u32)->i32;
        fn TranslateMessage(message:*const Message)->i32;
        fn DispatchMessageW(message:*const Message)->isize;
        fn GetClientRect(window:isize,rectangle:*mut Rectangle)->i32;
        fn MoveWindow(window:isize,x:i32,y:i32,width:i32,height:i32,repaint:i32)->i32;
        fn GetWindowTextLengthW(window:isize)->i32;
        fn GetWindowTextW(window:isize,text:*mut u16,maximum:i32)->i32;
        #[cfg_attr(target_pointer_width="32",link_name="SetWindowLongW")]
        fn SetWindowLongPtrW(window:isize,index:i32,value:isize)->isize;
        #[cfg_attr(target_pointer_width="32",link_name="GetWindowLongW")]
        fn GetWindowLongPtrW(window:isize,index:i32)->isize;
    }
    #[link(name="gdi32")]
    unsafe extern "system" { fn GetStockObject(index:i32)->isize; }

    fn wide(text: &str) -> Vec<u16> { text.encode_utf16().chain(Some(0)).collect() }
    fn send(window:isize,message:u32,first:usize,second:isize)->isize {
        unsafe { SendMessageW(window,message,first,second) }
    }
    fn window_error(operation:&str)->anyhow::Error {
        anyhow::anyhow!("{operation}: Win32 error {}",unsafe { GetLastError() })
    }

    struct State {
        events: Sender<Event>, list:isize, names:isize, query:isize, rows:Vec<MessageListRow>,
        speakers:Vec<Vec<u8>>, visible:Vec<usize>, encoding:ScriptTextEncoding,
    }

    impl State {
        fn layout(&self, window:isize) {
            let mut rect=Rectangle {left:0,top:0,right:0,bottom:0};
            unsafe {
                GetClientRect(window,&mut rect);
                MoveWindow(self.names,8,8,(rect.right/3).max(120),300,1);
                let x=(rect.right/3).max(120)+16;
                MoveWindow(self.query,x,8,(rect.right-x-8).max(20),26,1);
                MoveWindow(self.list,8,42,(rect.right-16).max(20),(rect.bottom-50).max(20),1);
            }
        }

        fn replace(&mut self, rows:Vec<MessageListRow>, encoding:ScriptTextEncoding)->Result<()> {
            self.rows=rows;
            self.encoding=encoding;
            self.speakers.clear();
            send(self.names,0x14b,0,0); // CB_RESETCONTENT
            for label in ["--All--","--Monologue--"] {
                send(self.names,0x143,0,wide(label).as_ptr() as isize);
            }
            for row in &self.rows {
                if !row.name.is_empty() && !self.speakers.contains(&row.name) {
                    self.speakers.push(row.name.clone());
                    send(self.names,0x143,0,wide(&encoding.decode(&row.name)).as_ptr() as isize);
                }
            }
            send(self.names,0x14e,0,0); // CB_SETCURSEL
            self.filter()
        }

        fn filter(&mut self)->Result<()> {
            let length=unsafe { GetWindowTextLengthW(self.query) };
            if length > 512 { bail!("message-list search exceeds 512 UTF16 units"); }
            let mut query=vec![0u16;(length.max(0)+1) as usize];
            let read=unsafe { GetWindowTextW(self.query,query.as_mut_ptr(),query.len() as i32) };
            let query=String::from_utf16(&query[..read.max(0) as usize])?;
            let bytes=match self.encoding {
                ScriptTextEncoding::Utf8=>query.into_bytes(),
                encoding=>{
                    let codec=if encoding==ScriptTextEncoding::Cp932 {encoding_rs::SHIFT_JIS}else{encoding_rs::GBK};
                    let (bytes,_,invalid)=codec.encode(&query);
                    if invalid {bail!("message-list search is not representable in the scene encoding");}
                    bytes.into_owned()
                }
            };
            let speaker=send(self.names,0x147,0,0); // CB_GETCURSEL
            let name=if speaker>=2 {self.speakers.get(speaker as usize-2).map(Vec::as_slice)} else {None};
            let selected=send(self.list,0x188,0,0); // LB_GETCURSEL
            let selected=self.visible.get(selected as usize).map(|&row|self.rows[row].input);
            send(self.list,0x184,0,0);
            self.visible.clear();
            for (index,row) in self.rows.iter().enumerate() {
                if !row.matches(name,speaker==1,&bytes) {continue;}
                let display=if row.name.is_empty() {
                    format!("\u{3000}{}",self.encoding.decode(&row.message))
                }else{
                    format!("{}{}",self.encoding.decode(&row.name),self.encoding.decode(&row.message))
                };
                let result=send(self.list,0x180,0,wide(&display).as_ptr() as isize);
                if result<0 {bail!("message-list control could not add its source row");}
                self.visible.push(index);
                send(self.list,0x19a,result as usize,row.input as isize);
                if selected==Some(row.input) {send(self.list,0x186,result as usize,0);}
            }
            Ok(())
        }
    }

    unsafe extern "system" fn procedure(window:isize,message:u32,first:usize,second:isize)->isize {
        let pointer=unsafe { GetWindowLongPtrW(window,-21) } as *const RefCell<State>;
        if pointer.is_null() {return unsafe {DefWindowProcW(window,message,first,second)};}
        // SendMessage can synchronously notify this parent while a row is
        // being rebuilt. RefCell denies a second mutable borrow safely.
        let Ok(mut state)=unsafe {&*pointer}.try_borrow_mut() else {return 0;};
        match message {
            0x10=>{ unsafe {ShowWindow(window,0)}; let _=state.events.send(Event::Hidden);0 }
            5=>{state.layout(window);0}
            0x111=>{
                let control=first & 0xffff;
                let notification=(first>>16)&0xffff;
                if (control==2 && notification==1)||(control==3 && notification==0x300) {
                    if let Err(cause)=state.filter() {let _=state.events.send(Event::Failure(format!("{cause:#}")));}
                }else if control==1 && notification==2 {
                    let selected=send(state.list,0x188,0,0);
                    if let Some(&index)=state.visible.get(selected as usize) {
                        let _=state.events.send(Event::Selected(state.rows[index].input));
                    }
                }
                0
            }
            _=>unsafe {DefWindowProcW(window,message,first,second)},
        }
    }

    struct Window(isize);
    impl Drop for Window { fn drop(&mut self) {unsafe {DestroyWindow(self.0)};} }

    fn run(requests:Receiver<Command>,events:&Sender<Event>,ready:&Sender<Result<()>>,title:&str)->Result<()> {
        let instance=unsafe {GetModuleHandleW(null())};
        let class=wide("CS2PortableMessageInspector");
        let descriptor=WindowClass {style:0,procedure:Some(procedure),class_extra:0,window_extra:0,
            instance,icon:0,cursor:0,background:6,menu:null(),class:class.as_ptr()};
        if unsafe {RegisterClassW(&descriptor)}==0 && unsafe {GetLastError()}!=1410 {
            return Err(window_error("register message-list window class"));
        }
        let state=Box::new(RefCell::new(State {
            events:events.clone(),list:0,names:0,query:0,rows:Vec::new(),speakers:Vec::new(),
            visible:Vec::new(),encoding:ScriptTextEncoding::Cp932,
        }));
        let handle=unsafe {CreateWindowExW(0x80,class.as_ptr(),wide(title).as_ptr(),
            0x00cf0000,120,120,800,600,0,0,instance,null())};
        if handle==0 {return Err(window_error("create message-list window"));}
        let window=Window(handle);
        unsafe {SetWindowLongPtrW(handle,-21,(&*state as *const RefCell<State>) as isize)};
        for (kind,id,style) in [("LISTBOX",1,0x00a00101),("COMBOBOX",2,0x00200103),("EDIT",3,0x00800080)] {
            let child=unsafe {CreateWindowExW(0,wide(kind).as_ptr(),wide("").as_ptr(),
                0x50000000|style,0,0,100,30,handle,id,instance,null())};
            if child==0 {return Err(window_error("create message-list control"));}
            send(child,0x30,unsafe {GetStockObject(17)} as usize,1);
            let mut state=state.borrow_mut();
            match id {1=>state.list=child,2=>state.names=child,_=>state.query=child}
        }
        state.borrow_mut().replace(Vec::new(),ScriptTextEncoding::Cp932)?;
        state.borrow().layout(handle);
        unsafe {ShowWindow(handle,5)};
        let _=ready.send(Ok(()));
        loop {
            loop {
                match requests.try_recv() {
                    Ok(Command::Rows(rows,encoding))=>state.borrow_mut().replace(rows,encoding)?,
                    Ok(Command::Visible(value))=>{unsafe {ShowWindow(handle,if value {5}else{0})};}
                    Ok(Command::Close)|Err(TryRecvError::Disconnected)=>{drop(window);return Ok(());}
                    Err(TryRecvError::Empty)=>break,
                }
            }
            // Limit one UI slice so owned host requests cannot starve behind
            // a busy Windows message queue. All waits stay on this UI thread.
            for _ in 0..64 {
                let mut message=Message {window:0,message:0,first:0,second:0,time:0,point:[0;2],private:0};
                if unsafe {PeekMessageW(&mut message,0,0,0,1)}==0 {break;}
                unsafe {TranslateMessage(&message);DispatchMessageW(&message);}
            }
            thread::sleep(Duration::from_millis(8));
        }
    }
}
