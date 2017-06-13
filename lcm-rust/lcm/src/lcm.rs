use std::io::{Error, ErrorKind, Result};
use std::ffi::CString;
use message::Message;
use std::cmp::Ordering;
use std::ptr;
use std::boxed::Box;
use std::sync::{Arc, Mutex};
use std::ops::Deref;
use std::slice;
use std::time::Duration;
use ffi::*;

/// An LCM instance that handles publishing and subscribing,
/// as well as encoding and decoding messages.
pub struct Lcm<'a> {
    lcm: *mut lcm_t,
    subscriptions: Mutex<Vec<Arc<LcmSubscription<'a>>>>,
}
unsafe impl<'a> Sync for Lcm<'a> {}
unsafe impl<'a> Send for Lcm<'a> {}


pub struct LcmSubscription<'a> {
    subscription: *mut lcm_subscription_t,
    handler: Box<FnMut(*const lcm_recv_buf_t) + 'a>,
}


impl<'a> Lcm<'a> {
    /// Creates a new `Lcm` instance.
    ///
    /// ```
    /// use lcm::Lcm;
    /// let lcm = Lcm::new().unwrap();
    /// ```
    pub fn new() -> Result<Lcm<'a>> {
        trace!("Creating LCM instance");
        let lcm = unsafe { lcm_create(ptr::null()) };
        match lcm.is_null() {
            true => Err(Error::new(ErrorKind::Other, "Failed to initialize LCM.")),
            false => {
                Ok(Lcm {
                    lcm: lcm,
                    subscriptions: Mutex::new(Vec::new()),
                })
            }
        }
    }

    pub fn get_fileno(&self) -> ::std::os::raw::c_int {
        unsafe { lcm_get_fileno(self.lcm) }
    }

    /// Subscribes a callback to a particular topic.
    ///
    /// ```
    /// # use lcm::Lcm;
    /// let lcm = Lcm::new().unwrap();
    /// lcm.subscribe("GREETINGS", |name: String| println!("Hello, {}!", name) );
    /// ```
    pub fn subscribe<M, F>(&self, channel: &str, mut callback: F) -> Arc<LcmSubscription<'a>>
        where M: Message,
              F: FnMut(M) + Send + 'a
    {
        trace!("Subscribing handler to channel {}", channel);

        let channel = CString::new(channel).unwrap();

        let handler = Box::new(move |rbuf: *const lcm_recv_buf_t| {
            trace!("Running handler");
            let mut buf = unsafe {
                let ref rbuf = *rbuf;
                let data = rbuf.data as *mut u8;
                let len = rbuf.data_size as usize;
                slice::from_raw_parts(data, len)
            };
            trace!("Decoding buffer: {:?}", buf);
            match M::decode_with_hash(&mut buf) {
                Ok(msg) => callback(msg),
                Err(_) => error!("Failed to decode buffer: {:?}", buf),
            }
        });

        let mut subscription = Arc::new(LcmSubscription {
            subscription: ptr::null_mut(),
            handler: handler,
        });

        let user_data = (subscription.deref() as *const _) as *mut _;

        let c_subscription = unsafe {
            lcm_subscribe(self.lcm,
                          channel.as_ptr(),
                          Some(Lcm::handler_callback::<M>),
                          user_data)
        };

        Arc::get_mut(&mut subscription).unwrap().subscription = c_subscription;
        let sub_clone = subscription.clone();
        self.subscriptions.lock().expect("Poisoned mutex").push(sub_clone);

        subscription
    }

    /// Unsubscribes a message handler.
    ///
    /// ```
    /// # use lcm::Lcm;
    /// # let handler_function = |name: String| println!("Hello, {}!", name);
    /// # let lcm = Lcm::new().unwrap();
    /// let handler = lcm.subscribe("GREETINGS", handler_function);
    /// // ...
    /// lcm.unsubscribe(handler);
    /// ```
    pub fn unsubscribe(&self, handler: Arc<LcmSubscription>) -> Result<()> {
        trace!("Unsubscribing handler {:?}", handler.subscription);
        let result = unsafe { lcm_unsubscribe(self.lcm, handler.subscription) };

        self.subscriptions.lock().expect("Poisoned mutex")
                          .retain(|sub| { sub.subscription != handler.subscription });

        match result {
            0 => Ok(()),
            _ => Err(Error::new(ErrorKind::Other, "LCM: Failed to unsubscribe")),
        }
    }

    /// Publishes a message on the specified channel.
    ///
    /// ```
    /// # use lcm::Lcm;
    /// let lcm = Lcm::new().unwrap();
    /// lcm.publish("GREETINGS", &"Charles".to_string()).unwrap();
    /// ```
    pub fn publish<M>(&self, channel: &str, message: &M) -> Result<()>
        where M: Message
    {
        let channel = CString::new(channel).unwrap();
        let buffer = message.encode_with_hash()?;
        let result = unsafe {
            lcm_publish(self.lcm,
                        channel.as_ptr(),
                        buffer.as_ptr() as *mut _,
                        buffer.len() as _)
        };
        match result {
            0 => Ok(()),
            _ => Err(Error::new(ErrorKind::Other, "LCM Error")),
        }
    }

    /// Waits for and dispatches the next incoming message.
    ///
    /// ```
    /// # use lcm::Lcm;
    /// # let handler_function = |name: String| println!("Hello, {}!", name);
    /// let lcm = Lcm::new().unwrap();
    /// lcm.subscribe("POSITION", handler_function);
    /// loop {
    /// # break;
    ///     lcm.handle().unwrap();
    /// }
    /// ```
    pub fn handle(&self) -> Result<()> {
        let result = unsafe { lcm_handle(self.lcm) };
        match result {
            0 => Ok(()),
            _ => Err(Error::new(ErrorKind::Other, "LCM Error")),
        }
    }

    /// Waits for and dispatches the next incoming message, up to a time limit.
    ///
    /// ```
    /// # use std::time::Duration;
    /// # use lcm::Lcm;
    /// # let handler_function = |name: String| println!("Hello, {}!", name);
    /// let lcm = Lcm::new().unwrap();
    /// lcm.subscribe("POSITION", handler_function);
    /// let wait_dur = Duration::from_millis(100);
    /// loop {
    /// # break;
    ///     lcm.handle_timeout(Duration::from_millis(1000)).unwrap();
    /// }
    /// ```
    pub fn handle_timeout(&self, timeout: Duration) -> Result<()> {
        let result = unsafe { lcm_handle_timeout(self.lcm, (timeout.as_secs() * 1000) as i32 + (timeout.subsec_nanos() / 1000_000) as i32) };
        match result.cmp(&0) {
            Ordering::Less => Err(Error::new(ErrorKind::Other, "LCM Error")),
            Ordering::Equal => Err(Error::new(ErrorKind::Other, "LCM Timeout")),
            Ordering::Greater => Ok(()),
        }
    }

    /// Adjusts the maximum number of received messages that can be queued up for a subscription.
    /// The default is `30`.
    ///
    /// ```
    /// # use lcm::Lcm;
    /// # let handler_function = |name: String| println!("Hello, {}!", name);
    /// # let lcm = Lcm::new().unwrap();
    /// let handler = lcm.subscribe("POSITION", handler_function);
    /// lcm.subscription_set_queue_capacity(handler, 30);
    /// ```
    pub fn subscription_set_queue_capacity(&self, handler: Arc<LcmSubscription>, num_messages: usize) {
        let handler = handler.subscription;
        let num_messages = num_messages as _;
        unsafe { lcm_subscription_set_queue_capacity(handler, num_messages) };
    }



    extern "C" fn handler_callback<M>(rbuf: *const lcm_recv_buf_t,
                                      _: *const ::std::os::raw::c_char,
                                      user_data: *mut ::std::os::raw::c_void)
        where M: Message
    {
        trace!("Received data");
        let sub = user_data as *mut LcmSubscription;
        let sub = unsafe { &mut *sub };
        (sub.handler)(rbuf);
    }
}

impl<'a> Drop for Lcm<'a> {
    fn drop(&mut self) {
        trace!("Destroying Lcm instance");
        unsafe { lcm_destroy(self.lcm) };
    }
}



#[cfg(test)]
///
/// Tests
///
mod test {
    use std::sync::Arc;
    use super::*;

    #[test]
    fn initialized() {
        let _lcm = Lcm::new().unwrap();
    }

    #[test]
    fn test_subscribe() {
        let lcm = Lcm::new().unwrap();
        lcm.subscribe("channel", |_: String| {});
        let subs = lcm.subscriptions.lock().unwrap();
        assert_eq!(subs.len(), 1);
    }

    #[test]
    fn test_unsubscribe() {
        let lcm = Lcm::new().unwrap();
        let sub = lcm.subscribe("channel", |_: String| {});
        lcm.unsubscribe(sub).unwrap();

        let subs = lcm.subscriptions.lock().unwrap();
        assert_eq!(subs.len(), 0);
    }
}
