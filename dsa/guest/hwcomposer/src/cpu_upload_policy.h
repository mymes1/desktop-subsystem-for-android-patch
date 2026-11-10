// Copyright 2026 The DSA Authors
// SPDX-License-Identifier: Apache-2.0
//
// Which layers the host composes, and when a layer holds a new picture, for the copies of
// what a CPU wrote into a buffer (cpu_upload.h). Free of Android headers, so the build
// machine tests it.

#pragma once

#include <cstdint>
#include <map>

namespace dsa::hwc {

// Whether the host may compose a layer of this DRM format itself. Its compositor samples
// RGB images only; a YUV layer (a video's pictures) needs a sampler conversion it does not
// make, so SurfaceFlinger draws it into the client target instead, from the same ColorBuffer.
bool hostComposes(uint32_t drmFourcc);

// The buffer each layer last showed. A layer whose buffer changes holds a new picture: a
// queue hands a buffer back to its producer only after it is shown, and the producer writes
// the next picture into it before queueing it again.
class LayerPictures {
   public:
    // Records that `layer` now shows `buffer`. True when that is another buffer than before.
    bool show(int64_t layer, uint32_t buffer);
    void forget(int64_t layer) { mShown.erase(layer); }

   private:
    std::map<int64_t, uint32_t> mShown;
};

}  // namespace dsa::hwc
