Received: from us-smtp-delivery-124.mimecast.com (us-smtp-delivery-124.mimecast.com [170.10.133.124])
	(using TLSv1.2 with cipher ECDHE-RSA-AES256-GCM-SHA384 (256/256 bits))
	(No client certificate requested)
	by smtp.subspace.kernel.org (Postfix) with ESMTPS id 3B8634A340A
	for <rust-for-linux@vger.kernel.org>; Tue,  3 Mar 2026 21:15:54 +0000 (UTC)
Authentication-Results: smtp.subspace.kernel.org; arc=none smtp.client-ip=170.10.133.124
ARC-Seal:i=1; a=rsa-sha256; d=subspace.kernel.org; s=arc-20240116;
	t=1772572556; cv=none; b=EnW2UrucfOEXbGIW2n9/uSasJ/55sh5Fe3YJ75cFvAi1gDCVYpVBty+rXWIOTRnJrWs1Sfe4s84XdfsCxDMbrwQTkC2yGp1LZ+zUyooTePDBl3rSNkpIPaVtm8w81j9SUsnVA6ZAUECcf/5Uy2IkKzW34802LpvHKKdLwHI6sDA=
ARC-Message-Signature:i=1; a=rsa-sha256; d=subspace.kernel.org;
	s=arc-20240116; t=1772572556; c=relaxed/simple;
	bh=aIeoCt7uogRc0fVC6+4FHa1SGMowdDhKrl+UTRXZUrA=;
	h=From:Date:Subject:MIME-Version:Message-Id:References:In-Reply-To:
	 To:Cc:Content-Type; b=uWcBfZOpdAMZY59VM9BNBsexKUPnu/iSzzCpQmQS+3cGt+iF19BBSO+nbwm4eR/1gSZkp2M9cs6UZVtYS3Lx8etgfgJv6pBCnjKbAzCN19IsGcNj5keZbXgFCoVBqmZ2dxxygj/DsDEVDvpT2r5o46/5ob+WHd5Yewy6PzoOkG0=
ARC-Authentication-Results:i=1; smtp.subspace.kernel.org; dmarc=pass (p=quarantine dis=none) header.from=redhat.com; spf=pass smtp.mailfrom=redhat.com; dkim=pass (1024-bit key) header.d=redhat.com header.i=@redhat.com header.b=ClSPKbLW; arc=none smtp.client-ip=170.10.133.124
Authentication-Results: smtp.subspace.kernel.org; dmarc=pass (p=quarantine dis=none) header.from=redhat.com
Authentication-Results: smtp.subspace.kernel.org; spf=pass smtp.mailfrom=redhat.com
Authentication-Results: smtp.subspace.kernel.org;
	dkim=pass (1024-bit key) header.d=redhat.com header.i=@redhat.com header.b="ClSPKbLW"
DKIM-Signature: v=1; a=rsa-sha256; c=relaxed/relaxed; d=redhat.com;
	s=mimecast20190719; t=1772572553;
	h=from:from:reply-to:subject:subject:date:date:message-id:message-id:
	 to:to:cc:cc:mime-version:mime-version:content-type:content-type:
	 content-transfer-encoding:content-transfer-encoding:
	 in-reply-to:in-reply-to:references:references;
	bh=62xgaFtJ6odD5MPfFUAjLdRekJQ/qnRaVs4d4H+RycE=;
	b=ClSPKbLWZNGN7BuHje3qvLZ7qVruJgZQlQ6Dzt90mRjvMLJOgyuwOe+iLQmRQtehu3M1Eu
	aSQSrjDKm+qViaKWCQQupOBQPImtlkLD1m9E9esi1+5kppZbDOfiVFmiuqnQNeh/b92YUz
	pxiThj/9P3Lw7yb/ihg7WMAwOf58mYs=
Received: from mail-qt1-f199.google.com (mail-qt1-f199.google.com
 [209.85.160.199]) by relay.mimecast.com with ESMTP with STARTTLS
 (version=TLSv1.3, cipher=TLS_AES_256_GCM_SHA384) id
 us-mta-416-soA5KRESOlOvJrmrAiRnfw-1; Tue, 03 Mar 2026 16:15:52 -0500
X-MC-Unique: soA5KRESOlOvJrmrAiRnfw-1
X-Mimecast-MFC-AGG-ID: soA5KRESOlOvJrmrAiRnfw_1772572552
Received: by mail-qt1-f199.google.com with SMTP id d75a77b69052e-5033b62efa7so456800871cf.1
        for <rust-for-linux@vger.kernel.org>; Tue, 03 Mar 2026 13:15:52 -0800 (PST)
X-Google-DKIM-Signature: v=1; a=rsa-sha256; c=relaxed/relaxed;
        d=1e100.net; s=20230601; t=1772572552; x=1773177352;
        h=cc:to:in-reply-to:references:message-id:content-transfer-encoding
         :mime-version:subject:date:from:x-gm-gg:x-gm-message-state:from:to
         :cc:subject:date:message-id:reply-to;
        bh=62xgaFtJ6odD5MPfFUAjLdRekJQ/qnRaVs4d4H+RycE=;
        b=KE5nNQSMuLEBv0PZzybca9KBEm0Ul7iKSRFV0b+oJQoDIfqXYi1m4C41Hv0u8XorbN
         nz33aaXYRm6wwSRQg8XOdoHx3q2UgOZJiWJd8jIfjGIHac8Y6xbHNBB85Vltkvnjc165
         nz7zR3SWy/N0TA3dGYtpxl/e/cRWvoSHgiGLv05rlf/g7c2bTdUCRjJuMMmNcHoUqyKq
         +dxgPNCOXxEVNHUsPGBc2ROS2Gv4o16+R59KJzU7CmsCiBSNUuJxXtLx0wT/QLYZzx+e
         gQafAEV1+xaC8CDftkkdLCVlUzo5RbP+oduPQBm36jbaPMTpj20SCuESpDLeb1SyQYvV
         7xCg==
X-Forwarded-Encrypted: i=1; AJvYcCVbLkjN4nYku9sGxdRsf2YECEa02M6ZmuOG2utDuzEfbLTXan2Av2pz0xnNVsJuoHD3RBHIXGxEs9fmE1NnCQ==@vger.kernel.org
X-Gm-Message-State: AOJu0Yzwny2+MHYHe1ko1mf+xC7r5rEFH6ydVmE5bpZAy2UFDzmo79FW
	/Vjlu6RaLv01zrU8jvrVx8Duescmn/lDwWujaMErlYhZeVD1QHnEb2mi0GWGhcU+O0BsJcIclZr
	X8E+m8Hy0G+p15lSSZxvnDuc8yRXhTSRuTwq+6Ycj4VwcfSf/LEMwrC+1cIH4hGoZnhHt
X-Gm-Gg: ATEYQzx+3ksS2y3KuwS2ck6fukhZ05HMCznH1hMWFWwDv1+j2vJqzxdiYGfpnm3sDTr
	E6e+qSMTT3a0E3dfijVLvGQfZx4tDA+N1ZpyL1vuwbmdcseq6r+LIbWJZN+sNUAlN5k4s15AlEq
	6ByBh9RlNdQZvXmj4rHDTe7FvOE9a/YKWHuVElFSWyDb6/LGfgb3d9xyHKa9KOEhh2NP3bLisSE
	ecuo4K97yh6aS7B37stqCaD8WVghHS+VqkyOiW11NYXkAx0nj0tuzwoywUWpaEL0CsdC1lL4JvY
	Q5yubqdZDCTzaqVUi7naz5WJkRFDydRynS6hXa71VztWqjkO1BJvCidF63H3DwkF9tRGas/Gdlf
	g9fVxgKJ5P5vogvOJ20UC8UNzKA==
X-Received: by 2002:ac8:7d53:0:b0:503:2ea0:ef2a with SMTP id d75a77b69052e-507528a83e7mr213703851cf.18.1772572551559;
        Tue, 03 Mar 2026 13:15:51 -0800 (PST)
X-Received: by 2002:ac8:7d53:0:b0:503:2ea0:ef2a with SMTP id d75a77b69052e-507528a83e7mr213703051cf.18.1772572551032;
        Tue, 03 Mar 2026 13:15:51 -0800 (PST)
Received: from [172.16.1.8] ([2607:f2c0:b1e3:9a00:3c7:56c2:f819:96d2])
        by smtp.gmail.com with ESMTPSA id d75a77b69052e-5074481c0e5sm156286991cf.0.2026.03.03.13.15.49
        (version=TLS1_3 cipher=TLS_AES_256_GCM_SHA384 bits=256/256);
        Tue, 03 Mar 2026 13:15:50 -0800 (PST)
From: Peter Colberg <pcolberg@redhat.com>
Date: Tue, 03 Mar 2026 16:15:29 -0500
Subject: [PATCH v3 09/10] rust: pci: add physfn(), to return PF device for
 VF device
Precedence: bulk
X-Mailing-List: rust-for-linux@vger.kernel.org
List-Id: <rust-for-linux.vger.kernel.org>
List-Subscribe: <mailto:rust-for-linux+subscribe@vger.kernel.org>
List-Unsubscribe: <mailto:rust-for-linux+unsubscribe@vger.kernel.org>
MIME-Version: 1.0
Message-Id: <20260303-rust-pci-sriov-v3-9-4443c35f0c88@redhat.com>
References: <20260303-rust-pci-sriov-v3-0-4443c35f0c88@redhat.com>
In-Reply-To: <20260303-rust-pci-sriov-v3-0-4443c35f0c88@redhat.com>
To: Danilo Krummrich <dakr@kernel.org>, Bjorn Helgaas <bhelgaas@google.com>, 
 =?utf-8?q?Krzysztof_Wilczy=C5=84ski?= <kwilczynski@kernel.org>, 
 Miguel Ojeda <ojeda@kernel.org>, Alex Gaynor <alex.gaynor@gmail.com>, 
 Gary Guo <gary@garyguo.net>, 
 =?utf-8?q?Bj=C3=B6rn_Roy_Baron?= <bjorn3_gh@protonmail.com>, 
 Benno Lossin <lossin@kernel.org>, Andreas Hindborg <a.hindborg@kernel.org>, 
 Alice Ryhl <aliceryhl@google.com>, Trevor Gross <tmgross@umich.edu>, 
 Abdiel Janulgue <abdiel.janulgue@gmail.com>, 
 Daniel Almeida <daniel.almeida@collabora.com>, 
 Robin Murphy <robin.murphy@arm.com>, 
 Greg Kroah-Hartman <gregkh@linuxfoundation.org>, 
 Dave Ertman <david.m.ertman@intel.com>, Ira Weiny <ira.weiny@intel.com>, 
 Leon Romanovsky <leon@kernel.org>, David Airlie <airlied@gmail.com>, 
 Simona Vetter <simona@ffwll.ch>, Jonathan Corbet <corbet@lwn.net>, 
 Xu Yilun <yilun.xu@intel.com>, Tom Rix <trix@redhat.com>, 
 Moritz Fischer <mdf@kernel.org>, "Rafael J. Wysocki" <rafael@kernel.org>, 
 Boqun Feng <boqun@kernel.org>
Cc: linux-pci@vger.kernel.org, rust-for-linux@vger.kernel.org, 
 linux-kernel@vger.kernel.org, Alexandre Courbot <acourbot@nvidia.com>, 
 Alistair Popple <apopple@nvidia.com>, 
 Joel Fernandes <joelagnelf@nvidia.com>, John Hubbard <jhubbard@nvidia.com>, 
 Zhi Wang <zhiw@nvidia.com>, nouveau@lists.freedesktop.org, 
 dri-devel@lists.freedesktop.org, linux-doc@vger.kernel.org, 
 linux-fpga@vger.kernel.org, driver-core@lists.linux.dev, 
 Peter Colberg <pcolberg@redhat.com>, Jason Gunthorpe <jgg@ziepe.ca>
X-Mailer: b4 0.14.2
X-Mimecast-Spam-Score: 0
X-Mimecast-MFC-PROC-ID: 6P6OG9GZ62sj5AQBSKe93cZclk6RASMBJNAxxSL3WtY_1772572552
X-Mimecast-Originator: redhat.com
Content-Type: text/plain; charset="utf-8"
Content-Transfer-Encoding: 7bit

Add a method to return the Physical Function (PF) device for a Virtual
Function (VF) device in the bound device context.

Unlike for a PCI driver written in C, guarantee that when a VF device is
bound to a driver, the underlying PF device is bound to a driver, too,
by always setting the flag managed_sriov in the pci_driver structure.

In case SR-IOV has been enabled by a C driver that has not set the flag
managed_sriov in pci_driver, return an error from physfn().

This change depends on commit a995fe1a3aa7 ("rust: driver: drop device
private data post unbind") to also uphold the safety guarantee in case
a (broken) PF driver re-enables SR-IOV in its unbind() callback. That
commit extends the lifetime of the device private data beyond the
remove_callback() wrapper. In particular, that commit ensures that the
device private data for the PF device is still alive until after the
function pci_iov_remove() is called and forcibly re-disables SR-IOV,
which means the data can be safely accessed by VF drivers until then.

Suggested-by: Danilo Krummrich <dakr@kernel.org>
Signed-off-by: Peter Colberg <pcolberg@redhat.com>
---
Changes in v3:
- Replace SR_IOV -> SR-IOV in description.

Changes in v2:
- Uphold safety guarantee when PF driver is written in C.
- Let physfn() return error if driver flag managed_sriov is unset.
---
 rust/kernel/pci.rs | 53 +++++++++++++++++++++++++++++++++++++++++++++++++++++
 1 file changed, 53 insertions(+)

diff --git a/rust/kernel/pci.rs b/rust/kernel/pci.rs
index 581930d0afe98ccc29d729e4d9aab75b4144e46c..3b11f73a9f2b69a02fe003b8feadd61864adc8c0 100644
--- a/rust/kernel/pci.rs
+++ b/rust/kernel/pci.rs
@@ -525,6 +525,59 @@ pub fn pci_class(&self) -> Class {
     }
 }
 
+impl Device<device::Bound> {
+    /// Returns the Physical Function (PF) device for a Virtual Function (VF) device.
+    ///
+    /// # Examples
+    ///
+    /// The following example illustrates how to obtain the private driver data of the PF device,
+    /// where `vf_pdev` is the VF device of reference type `&Device<Core>` or `&Device<Bound>`.
+    ///
+    /// ```
+    /// # use kernel::{device::Core, pci};
+    /// /// A PCI driver that binds to both the PF and its VF devices.
+    /// struct MyDriver;
+    ///
+    /// impl MyDriver {
+    ///     fn connect(vf_pdev: &pci::Device<Core>) -> Result {
+    ///         let pf_pdev = vf_pdev.physfn()?;
+    ///         let pf_drvdata = pf_pdev.as_ref().drvdata::<Self>()?;
+    ///         Ok(())
+    ///     }
+    /// }
+    /// ```
+    #[cfg(CONFIG_PCI_IOV)]
+    pub fn physfn(&self) -> Result<&Device<device::Bound>> {
+        if !self.is_virtfn() {
+            return Err(EINVAL);
+        }
+
+        // SAFETY: `self.as_raw()` returns a valid pointer to a `struct pci_dev`.
+        // `physfn` is a valid pointer to a `struct pci_dev` since `is_virtfn()` is `true`.
+        let pf_dev = unsafe { (*self.as_raw()).__bindgen_anon_1.physfn };
+
+        // SAFETY: `pf_dev` is a valid pointer to a `struct pci_dev`.
+        // `driver` is either NULL or a valid pointer to a `struct pci_driver`.
+        let pf_drv = unsafe { (*pf_dev).driver };
+        if pf_drv.is_null() {
+            return Err(EINVAL);
+        }
+
+        // SAFETY: `pf_drv` is a valid pointer to a `struct pci_driver`.
+        if !unsafe { (*pf_drv).managed_sriov } {
+            return Err(EINVAL);
+        }
+
+        // SAFETY: `physfn` may be cast to a `Device<device::Bound>` since the
+        // driver flag `managed_sriov` forces SR-IOV to be disabled when the
+        // PF driver is unbound, i.e., all VF devices are destroyed. This
+        // guarantees that the underlying PF device is bound to a driver
+        // when the VF device is bound to a driver, which is the case since
+        // `Device::physfn()` requires a `&Device<Bound>` reference.
+        Ok(unsafe { &*pf_dev.cast() })
+    }
+}
+
 impl Device<device::Core> {
     /// Enable memory resources for this device.
     pub fn enable_device_mem(&self) -> Result {

-- 
2.53.0

