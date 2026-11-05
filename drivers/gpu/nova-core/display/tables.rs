// SPDX-License-Identifier: MIT

/*
 * Copyright (c) 2010-2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
 *
 * Permission is hereby granted, free of charge, to any person obtaining a
 * copy of this software and associated documentation files (the "Software"),
 * to deal in the Software without restriction, including without limitation
 * the rights to use, copy, modify, merge, publish, distribute, sublicense,
 * and/or sell copies of the Software, and to permit persons to whom the
 * Software is furnished to do so, subject to the following conditions:
 *
 * The above copyright notice and this permission notice shall be included in
 * all copies or substantial portions of the Software.
 *
 * THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
 * IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
 * FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL
 * THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
 * LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
 * FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
 * DEALINGS IN THE SOFTWARE.
 */

//! Shared display scaler coefficients.
//!
//! From OpenRM 615.71.09, `src/nvidia-modeset/src/nvkms-evo3.c`.

pub(super) const SCALER_5TAP_COEFFICIENTS: [[[i16; 4]; 16]; 3] = [
    [
        [0, 0, -16, 144],
        [0, -5, 5, 0],
        [0, -9, 11, 0],
        [-1, -12, 18, -1],
        [-1, -15, 25, -1],
        [-1, -18, 33, -2],
        [-2, -20, 42, -3],
        [-2, -21, 51, -3],
        [-3, -22, 60, -5],
        [-3, -22, 70, -6],
        [-4, -22, 81, -7],
        [-4, -22, 91, -9],
        [-5, -21, 102, -10],
        [-5, -20, 113, -12],
        [-5, -19, 125, -13],
        [-6, -18, 136, -15],
    ],
    [
        [3, 60, 20, 108],
        [3, 57, 63, 4],
        [2, 54, 66, 4],
        [2, 51, 69, 5],
        [2, 48, 72, 6],
        [1, 45, 75, 7],
        [1, 43, 78, 7],
        [1, 40, 81, 8],
        [1, 37, 84, 9],
        [0, 35, 88, 10],
        [0, 33, 91, 12],
        [0, 30, 94, 13],
        [0, 28, 97, 14],
        [0, 26, 99, 16],
        [0, 24, 102, 17],
        [0, 22, 105, 19],
    ],
    [
        [4, 62, 23, 105],
        [4, 59, 64, 5],
        [3, 56, 67, 6],
        [3, 53, 70, 7],
        [2, 51, 73, 8],
        [2, 48, 76, 8],
        [2, 45, 79, 9],
        [1, 43, 81, 10],
        [1, 40, 84, 12],
        [1, 38, 87, 13],
        [1, 36, 90, 14],
        [0, 34, 92, 15],
        [0, 31, 95, 17],
        [0, 29, 97, 18],
        [0, 27, 100, 20],
        [0, 25, 102, 22],
    ],
];
