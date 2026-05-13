(module
  (import "wasi_snapshot_preview1" "path_link"
    (func $path_link (param i32 i32 i32 i32 i32 i32 i32) (result i32)))
  (import "wasi_snapshot_preview1" "proc_exit"
    (func $proc_exit (param i32)))

  (memory (export "memory") 1)
  (data (i32.const 64) "source.txt")
  (data (i32.const 96) "linked.txt")

  (func $assert_ok (param $errno i32) (param $code i32)
    local.get $errno
    i32.eqz
    if
    else
      local.get $code
      call $proc_exit
    end)

  (func (export "_start")
    (call $assert_ok
      (call $path_link
        (i32.const 3)
        (i32.const 0)
        (i32.const 64)
        (i32.const 10)
        (i32.const 3)
        (i32.const 96)
        (i32.const 10))
      (i32.const 10)))
)
