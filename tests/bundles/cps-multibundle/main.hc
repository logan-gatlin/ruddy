extern mark : String -> () = "globalThis.host.mark"
let ready = do
  let result = dep::wait 20.0
  let _ = mark "root:ready"
  return result
end
effect Ask = Real -> Real
let read = fn n => handle (1.0 + !Ask n) with | !Ask value => dep::wait value end
