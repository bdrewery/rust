// Copyright 2015 The Rust Project Developers. See the COPYRIGHT
// file at the top-level directory of this distribution and at
// http://rust-lang.org/COPYRIGHT.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

// rsbegin.o and rsend.o are the so called "compiler runtime startup objects".
// They contain code needed to correctly initialize the compiler runtime.
//
// When an executable or dylib image is linked, all user code and libraries are
// "sandwiched" between these two object files, so code or data from rsbegin.o
// become first in the respective sections of the image, whereas code and data
// from rsend.o become the last ones.  This effect can be used to place symbols
// at the beginning or at the end of a section, as well as to insert any required
// headers or footers.
//
// Note that the actual module entry point is located in the C runtime startup
// object (usually called `crtX.o), which then invokes initialization callbacks
// of other runtime components (registered via yet another special image section).

#![crate_type="rlib"]
#![no_std]
#![allow(non_camel_case_types)]
