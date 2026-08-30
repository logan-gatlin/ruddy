effect Fail = { abort: () -> () }
type Callback = () -> () + !Fail
extern install : fn(Callback) -> () = host.install
