let map : ('a -> 'b + ..'effects) -> Result 'a 'error -> Result 'b 'error + ..'effects =
  fn function result => match result with
  | #Some value => #Some (function value)
  | #Error error => #Error error
  end

let map_error : ('a -> 'b + ..'effects) -> Result 'some 'a -> Result 'some 'b + ..'effects =
  fn function result => match result with
  | #Some value => #Some value
  | #Error error => #Error (function error)
  end

let and_then : ('a -> Result 'b 'error + ..'effects) -> Result 'a 'error -> Result 'b 'error + ..'effects =
  fn function result => match result with
  | #Some value => function value
  | #Error error => #Error error
  end

let unwrap_or : 'a -> Result 'a 'error -> 'a = fn fallback result => match result with
  | #Some value => value
  | #Error _ => fallback
end

let or_else : ('a -> Result 'some 'b + ..'effects) -> Result 'some 'a -> Result 'some 'b + ..'effects =
  fn fallback result => match result with
  | #Some value => #Some value
  | #Error error => fallback error
  end

let is_ok : Result 'some 'error -> Boolean = fn result => match result with
  | #Some _ => true
  | #Error _ => false
end

let is_error : Result 'some 'error -> Boolean = fn result => match result with
  | #Some _ => false
  | #Error _ => true
end

let to_option : Result 'some 'error -> Option 'some = fn result => match result with
  | #Some value => #Some value
  | #Error _ => #None
end

let error_or : 'error -> (#Error 'error | ..'rest) -> 'error = fn fallback value => match value with
  | #Error error => error
  | _ => fallback
end

let is_error_case : (#Error 'error | ..'rest) -> Boolean = fn value => match value with
  | #Error _ => true
  | _ => false
end
