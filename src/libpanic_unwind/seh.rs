// Copyright 2015 The Rust Project Developers. See the COPYRIGHT
// file at the top-level directory of this distribution and at
// http://rust-lang.org/COPYRIGHT.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

//! Windows SEH
//!
//! On Windows (currently only on MSVC), the default exception handling
//! mechanism is Structured Exception Handling (SEH). This is quite different
//! than Dwarf-based exception handling (e.g. what other unix platforms use) in
//! terms of compiler internals, so LLVM is required to have a good deal of
//! extra support for SEH.
//!
//! In a nutshell, what happens here is:
//!
//! 1. The `panic` function calls the standard Windows function `RaiseException`
//!    with a Rust-specific code, triggering the unwinding process.
//! 2. All landing pads generated by the compiler use the personality function
//!    `__C_specific_handler` on 64-bit and `__except_handler3` on 32-bit,
//!    functions in the CRT, and the unwinding code in Windows will use this
//!    personality function to execute all cleanup code on the stack.
//! 3. All compiler-generated calls to `invoke` have a landing pad set as a
//!    `cleanuppad` LLVM instruction, which indicates the start of the cleanup
//!    routine. The personality (in step 2, defined in the CRT) is responsible
//!    for running the cleanup routines.
//! 4. Eventually the "catch" code in the `try` intrinsic (generated by the
//!    compiler) is executed, which will ensure that the exception being caught
//!    is indeed a Rust exception, indicating that control should come back to
//!    Rust. This is done via a `catchswitch` plus a `catchpad` instruction in
//!    LLVM IR terms, finally returning normal control to the program with a
//!    `catchret` instruction. The `try` intrinsic uses a filter function to
//!    detect what kind of exception is being thrown, and this detection is
//!    implemented as the msvc_try_filter language item below.
//!
//! Some specific differences from the gcc-based exception handling are:
//!
//! * Rust has no custom personality function, it is instead *always*
//!   __C_specific_handler or __except_handler3, so the filtering is done in a
//!   C++-like manner instead of in the personality function itself. Note that
//!   the precise codegen for this was lifted from an LLVM test case for SEH
//!   (this is the `__rust_try_filter` function below).
//! * We've got some data to transmit across the unwinding boundary,
//!   specifically a `Box<Any + Send>`. Like with Dwarf exceptions
//!   these two pointers are stored as a payload in the exception itself. On
//!   MSVC, however, there's no need for an extra allocation because the call
//!   stack is preserved while filter functions are being executed. This means
//!   that the pointers are passed directly to `RaiseException` which are then
//!   recovered in the filter function to be written to the stack frame of the
//!   `try` intrinsic.
//!
//! [win64]: http://msdn.microsoft.com/en-us/library/1eyas8tf.aspx
//! [llvm]: http://llvm.org/docs/ExceptionHandling.html#background-on-windows-exceptions

#![allow(bad_style)]
#![allow(private_no_mangle_fns)]

use alloc::boxed::Box;
use core::any::Any;
use core::mem;
use core::raw;

use windows as c;

// A code which indicates panics that originate from Rust. Note that some of the
// upper bits are used by the system so we just set them to 0 and ignore them.
//                           0x 0 R S T
const RUST_PANIC: c::DWORD = 0x00525354;

pub unsafe fn panic(data: Box<Any + Send>) -> u32 {
    // As mentioned above, the call stack here is preserved while the filter
    // functions are running, so it's ok to pass stack-local arrays into
    // `RaiseException`.
    //
    // The two pointers of the `data` trait object are written to the stack,
    // passed to `RaiseException`, and they're later extracted by the filter
    // function below in the "custom exception information" section of the
    // `EXCEPTION_RECORD` type.
    let ptrs = mem::transmute::<_, raw::TraitObject>(data);
    let ptrs = [ptrs.data, ptrs.vtable];
    c::RaiseException(RUST_PANIC, 0, 2, ptrs.as_ptr() as *mut _);
    u32::max_value()
}

pub fn payload() -> [usize; 2] {
    [0; 2]
}

pub unsafe fn cleanup(payload: [usize; 2]) -> Box<Any + Send> {
    mem::transmute(raw::TraitObject {
        data: payload[0] as *mut _,
        vtable: payload[1] as *mut _,
    })
}

// This is quite a special function, and it's not literally passed in as the
// filter function for the `catchpad` of the `try` intrinsic. The compiler
// actually generates its own filter function wrapper which will delegate to
// this for the actual execution logic for whether the exception should be
// caught. The reasons for this are:
//
// * Each architecture has a slightly different ABI for the filter function
//   here. For example on x86 there are no arguments but on x86_64 there are
//   two.
// * This function needs access to the stack frame of the `try` intrinsic
//   which is using this filter as a catch pad. This is because the payload
//   of this exception, `Box<Any>`, needs to be transmitted to that
//   location.
//
// Both of these differences end up using a ton of weird llvm-specific
// intrinsics, so it's actually pretty difficult to express the entire
// filter function in Rust itself. As a compromise, the compiler takes care
// of all the weird LLVM-specific and platform-specific stuff, getting to
// the point where this function makes the actual decision about what to
// catch given two parameters.
//
// The first parameter is `*mut EXCEPTION_POINTERS` which is some contextual
// information about the exception being filtered, and the second pointer is
// `*mut *mut [usize; 2]` (the payload here). This value points directly
// into the stack frame of the `try` intrinsic itself, and we use it to copy
// information from the exception onto the stack.
#[lang = "msvc_try_filter"]
unsafe extern fn __rust_try_filter(eh_ptrs: *mut u8,
                                   payload: *mut u8) -> i32 {
    let eh_ptrs = eh_ptrs as *mut c::EXCEPTION_POINTERS;
    let payload = payload as *mut *mut [usize; 2];
    let record = &*(*eh_ptrs).ExceptionRecord;
    if record.ExceptionCode != RUST_PANIC {
        return 0
    }
    (**payload)[0] = record.ExceptionInformation[0] as usize;
    (**payload)[1] = record.ExceptionInformation[1] as usize;
    return 1
}

#[lang = "eh_personality"]
#[cfg(target_arch = "x86_64")]
#[no_mangle]
#[allow(unused)]
unsafe extern fn rust_seh64_personality(
    ExceptionRecord: *mut c::EXCEPTION_RECORD,
    EstablisherFrame: *mut u8,
    ContextRecord: *mut u8,
    DispatcherContext: *mut u8,
) -> c::EXCEPTION_DISPOSITION {
    if (*ExceptionRecord).ExceptionCode != RUST_PANIC {
        c::ExceptionContinueSearch
    } else {
        c::__C_specific_handler(ExceptionRecord,
                                EstablisherFrame,
                                ContextRecord,
                                DispatcherContext)
    }
}

#[lang = "eh_personality"]
#[cfg(target_arch = "x86")]
#[no_mangle]
#[allow(unused)]
unsafe extern fn rust_seh32_personality(
   exception_record: *mut c::EXCEPTION_RECORD,
   registration: *mut u8,
   context: *mut u8,
   dispatcher: *mut u8,
) -> i32 {
    if (*exception_record).ExceptionCode != RUST_PANIC &&
       (*exception_record).ExceptionCode != c::STATUS_UNWIND
    {
        c::DISPOSITION_CONTINUE_SEARCH
    } else {
        c::_except_handler3(exception_record,
                            registration,
                            context,
                            dispatcher)
    }
}
