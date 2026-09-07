let invoke : () -> Nat = fn _ => do
  let result : () = with_platform (fn _ => program::main ())
  return 0n
end
