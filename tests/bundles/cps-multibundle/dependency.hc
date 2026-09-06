@ffi { completion: "promise" }
extern delay : Real -> Real = "globalThis.host.delay"
extern mark : String -> () = "globalThis.host.mark"
let ready = do
  let _ = mark "dep:start"
  let result = delay 10.0
  let _ = mark "dep:ready"
  return result
end
let wait = fn value => delay value
