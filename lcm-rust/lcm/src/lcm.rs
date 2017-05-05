use error::{ErrorKind, Result};
use std::ffi::CString;
use message::Message;
use std::cmp::Ordering;
use std::ptr;
use std::boxed::Box;
use std::rc::Rc;
use std::ops::Deref;
use std::slice;
use std::time::Duration;
use ffi::*;

/// An LCM instance that handles publishing and subscribing,
/// as well as encoding and decoding messages.
pub struct Lcm {
    lcm: *mut lcm_t,
    subscriptions: Vec<Rc<LcmSubscription>>,
}


pub struct LcmSubscription {
    subscription: *mut lcm_subscription_t,
    handler: Box<FnMut(*const lcm_recv_buf_t)>,
}


impl Lcm {
    /// Creates a new `Lcm` instance.
    ///
    /// ```
    /// use lcm::Lcm;
    /// let mut lcm = Lcm::new().unwrap();
    /// ```
    pub fn new() -> Result<Lcm> {
        trace!("Creating LCM instance");
        let lcm = unsafe { lcm_create(ptr::null()) };
        match lcm.is_null() {
            true => Err(ErrorKind::FailedToInitialize.into()),
            false => {
                Ok(Lcm {
                    lcm: lcm,
                    subscriptions: Vec::new(),
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
    /// let mut lcm = Lcm::new().unwrap();
    /// lcm.subscribe("GREETINGS", |name: String| println!("Hello, {}!", name) );
    /// ```
    pub fn subscribe<M, F>(&mut self, channel: &str, mut callback: F) -> Rc<LcmSubscription>
        where M: Message,
              F: FnMut(M) + 'static
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

        let mut subscription = Rc::new(LcmSubscription {
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

        Rc::get_mut(&mut subscription).unwrap().subscription = c_subscription;
        self.subscriptions.push(subscription.clone());

        subscription
    }

    /// Unsubscribes a message handler.
    ///
    /// ```
    /// # use lcm::Lcm;
    /// # let handler_function = |name: String| println!("Hello, {}!", name);
    /// # let mut lcm = Lcm::new().unwrap();
    /// let handler = lcm.subscribe("GREETINGS", handler_function);
    /// // ...
    /// lcm.unsubscribe(handler);
    /// ```
    pub fn unsubscribe(&mut self, handler: Rc<LcmSubscription>) -> Result<()> {
        trace!("Unsubscribing handler {:?}", handler.subscription);
        let result = unsafe { lcm_unsubscribe(self.lcm, handler.subscription) };

        self.subscriptions.retain(|sub| { sub.subscription != handler.subscription });

        match result {
            0 => Ok(()),
            _ => Err(ErrorKind::FailedToUnsubscribe.into())
        }
    }

    /// Publishes a message on the specified channel.
    ///
    /// ```
    /// # use lcm::Lcm;
    /// let mut lcm = Lcm::new().unwrap();
    /// lcm.publish("GREETINGS", &"Charles".to_string()).unwrap();
    /// ```
    pub fn publish<M>(&mut self, channel: &str, message: &M) -> Result<()>
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
            _ => Err(ErrorKind::FailedToPublish.into())
        }
    }

    /// Waits for and dispatches the next incoming message.
    ///
    /// ```
    /// # use lcm::Lcm;
    /// # let handler_function = |name: String| println!("Hello, {}!", name);
    /// let mut lcm = Lcm::new().unwrap();
    /// lcm.subscribe("POSITION", handler_function);
    /// loop {
    /// # break;
    ///     lcm.handle().unwrap();
    /// }
    /// ```
    pub fn handle(&mut self) -> Result<()> {
        let result = unsafe { lcm_handle(self.lcm) };
        match result {
            0 => Ok(()),
            _ => Err(ErrorKind::InternalError.into())
        }
    }

    /// Waits for and dispatches the next incoming message, up to a time limit.
    ///
    /// ```
    /// # use std::time::Duration;
    /// # use lcm::Lcm;
    /// # let handler_function = |name: String| println!("Hello, {}!", name);
    /// let mut lcm = Lcm::new().unwrap();
    /// lcm.subscribe("POSITION", handler_function);
    /// let wait_dur = Duration::from_millis(100);
    /// loop {
    /// # break;
    ///     lcm.handle_timeout(Duration::from_millis(1000)).unwrap();
    /// }
    /// ```
    pub fn handle_timeout(&mut self, timeout: Duration) -> Result<()> {
        let result = unsafe { lcm_handle_timeout(self.lcm, (timeout.as_secs() * 1000) as i32 + (timeout.subsec_nanos() / 1000_000) as i32) };
        match result.cmp(&0) {
            Ordering::Less => Err(ErrorKind::InternalError.into()),
            Ordering::Equal => Err(ErrorKind::Timeout.into()),
            Ordering::Greater => Ok(()),
        }
    }

    /// Adjusts the maximum number of received messages that can be queued up for a subscription.
    /// The default is `30`.
    ///
    /// ```
    /// # use lcm::Lcm;
    /// # let handler_function = |name: String| println!("Hello, {}!", name);
    /// # let mut lcm = Lcm::new().unwrap();
    /// let handler = lcm.subscribe("POSITION", handler_function);
    /// lcm.subscription_set_queue_capacity(handler, 30);
    /// ```
    pub fn subscription_set_queue_capacity(&self, handler: Rc<LcmSubscription>, num_messages: usize) {
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

impl Drop for Lcm {
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
    use super::*;

    #[test]
    fn initialized() {
        let _lcm = Lcm::new().unwrap();
    }

    #[test]
    fn test_subscribe() {
        let mut lcm = Lcm::new().unwrap();
        lcm.subscribe("channel", |_: String| {});
        assert_eq!(lcm.subscriptions.len(), 1);
    }

    #[test]
    fn test_unsubscribe() {
        let mut lcm = Lcm::new().unwrap();
        let sub = lcm.subscribe("channel", |_: String| {});
        lcm.unsubscribe(sub).unwrap();
        assert_eq!(lcm.subscriptions.len(), 0);
    }
}
