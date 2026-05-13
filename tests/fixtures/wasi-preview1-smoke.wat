(module
  (import "wasi_snapshot_preview1" "args_sizes_get"
    (func $args_sizes_get (param i32 i32) (result i32)))
  (import "wasi_snapshot_preview1" "environ_sizes_get"
    (func $environ_sizes_get (param i32 i32) (result i32)))
  (import "wasi_snapshot_preview1" "clock_res_get"
    (func $clock_res_get (param i32 i32) (result i32)))
  (import "wasi_snapshot_preview1" "clock_time_get"
    (func $clock_time_get (param i32 i64 i32) (result i32)))
  (import "wasi_snapshot_preview1" "random_get"
    (func $random_get (param i32 i32) (result i32)))
  (import "wasi_snapshot_preview1" "fd_write"
    (func $fd_write (param i32 i32 i32 i32) (result i32)))
  (import "wasi_snapshot_preview1" "proc_exit"
    (func $proc_exit (param i32)))

  (memory (export "memory") 1)
  (data (i32.const 256) "compiled-wasi-ok\n")

  (func $assert_ok (param $errno i32) (param $code i32)
    local.get $errno
    i32.eqz
    if
    else
      local.get $code
      call $proc_exit
    end)

  (func $assert_i32 (param $actual i32) (param $want i32) (param $code i32)
    local.get $actual
    local.get $want
    i32.eq
    if
    else
      local.get $code
      call $proc_exit
    end)

  (func (export "_start")
    (call $assert_ok
      (call $clock_res_get
        (i32.const 0)
        (i32.const 0))
      (i32.const 10))
    (call $assert_ok
      (call $clock_time_get
        (i32.const 1)
        (i64.const 0)
        (i32.const 8))
      (i32.const 11))
    (call $assert_ok
      (call $random_get
        (i32.const 32)
        (i32.const 8))
      (i32.const 12))
    (call $assert_ok
      (call $args_sizes_get
        (i32.const 48)
        (i32.const 52))
      (i32.const 13))
    (call $assert_ok
      (call $environ_sizes_get
        (i32.const 56)
        (i32.const 60))
      (i32.const 14))

    (i32.store (i32.const 64) (i32.const 256))
    (i32.store (i32.const 68) (i32.const 17))
    (call $assert_ok
      (call $fd_write
        (i32.const 1)
        (i32.const 64)
        (i32.const 1)
        (i32.const 72))
      (i32.const 15))
    (call $assert_i32
      (i32.load (i32.const 72))
      (i32.const 17)
      (i32.const 16)))
)
