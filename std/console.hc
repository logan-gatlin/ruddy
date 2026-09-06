let write = !Console.write
let write_error = !Console.write_error

let print = fn text => write (str::concat text "\n")
let print_error = fn text => write_error (str::concat text "\n")
