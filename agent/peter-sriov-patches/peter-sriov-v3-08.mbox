Received: from us-smtp-delivery-124.mimecast.com (us-smtp-delivery-124.mimecast.com [170.10.129.124])
	(using TLSv1.2 with cipher ECDHE-RSA-AES256-GCM-SHA384 (256/256 bits))
	(No client certificate requested)
	by smtp.subspace.kernel.org (Postfix) with ESMTPS id E1ACF3DEAC7
	for <rust-for-linux@vger.kernel.org>; Tue,  3 Mar 2026 21:15:51 +0000 (UTC)
Authentication-Results: smtp.subspace.kernel.org; arc=none smtp.client-ip=170.10.129.124
ARC-Seal:i=1; a=rsa-sha256; d=subspace.kernel.org; s=arc-20240116;
	t=1772572554; cv=none; b=pl6B2y95Shnfnq5JruhtKDYFjO8GrbHp0iZRNkZv1KpuzYbLcEGN1tiKNJBcWUhNIgJHjU+/BUr3Sf4mkWDOoCBfzlVslaQCPXyLZmW2rsYFWXnPZ5amXK/4nAerqPBVkrqyvR/hRams7tK0IE7eJ2+YpPgg5UW7oujOKee6v8o=
ARC-Message-Signature:i=1; a=rsa-sha256; d=subspace.kernel.org;
	s=arc-20240116; t=1772572554; c=relaxed/simple;
	bh=njU3I9YW/zDKjP+0RxMLdIuH1MmdjiC7YPBjwAwiJ4w=;
	h=From:Date:Subject:MIME-Version:Message-Id:References:In-Reply-To:
	 To:Cc:Content-Type; b=i3iRsRbiIio98YfJO8prD5HTYsqa8u8b5/22Ulnrq+7ej/Y4TXypG9IzSrcdg/m14m8QcCEGcIBvfHnWn/I5RxL6Od3WMaRe2E7BFLplB+9rM8uUmrwfjT74MDaamH9zfOCfP9kWSBnj1IhBcnKDxsJYKjjnBYGEdLkEmelK8Sw=
ARC-Authentication-Results:i=1; smtp.subspace.kernel.org; dmarc=pass (p=quarantine dis=none) header.from=redhat.com; spf=pass smtp.mailfrom=redhat.com; dkim=pass (1024-bit key) header.d=redhat.com header.i=@redhat.com header.b=dwcPEOXW; arc=none smtp.client-ip=170.10.129.124
Authentication-Results: smtp.subspace.kernel.org; dmarc=pass (p=quarantine dis=none) header.from=redhat.com
Authentication-Results: smtp.subspace.kernel.org; spf=pass smtp.mailfrom=redhat.com
Authentication-Results: smtp.subspace.kernel.org;
	dkim=pass (1024-bit key) header.d=redhat.com header.i=@redhat.com header.b="dwcPEOXW"
DKIM-Signature: v=1; a=rsa-sha256; c=relaxed/relaxed; d=redhat.com;
	s=mimecast20190719; t=1772572551;
	h=from:from:reply-to:subject:subject:date:date:message-id:message-id:
	 to:to:cc:cc:mime-version:mime-version:content-type:content-type:
	 content-transfer-encoding:content-transfer-encoding:
	 in-reply-to:in-reply-to:references:references;
	bh=Nsl81xXoWOrG7fZ6D3nnOVMYFTWE73QzzhAJNIpS3Uk=;
	b=dwcPEOXWn4opOtp5G4RGm5qe6Qz8rm7pfGdh0iMeUmwtKZG1yxHkIjnz/6sJhozPAg+jGw
	BPhmntvzfQDpKMFQMutl5w4Jxup0LEhbRpr7gCvdTgViYsgeXzt/BoyBXL0C3zI7P4NxJL
	YVXY5CJRUTZn9VOUgJbM7LkXFNyw/XU=
Received: from mail-qt1-f198.google.com (mail-qt1-f198.google.com
 [209.85.160.198]) by relay.mimecast.com with ESMTP with STARTTLS
 (version=TLSv1.3, cipher=TLS_AES_256_GCM_SHA384) id
 us-mta-407-oWMZBywtMfuEWEBb7YMZFQ-1; Tue, 03 Mar 2026 16:15:50 -0500
X-MC-Unique: oWMZBywtMfuEWEBb7YMZFQ-1
X-Mimecast-MFC-AGG-ID: oWMZBywtMfuEWEBb7YMZFQ_1772572550
Received: by mail-qt1-f198.google.com with SMTP id d75a77b69052e-503915b0a88so661323231cf.1
        for <rust-for-linux@vger.kernel.org>; Tue, 03 Mar 2026 13:15:50 -0800 (PST)
X-Google-DKIM-Signature: v=1; a=rsa-sha256; c=relaxed/relaxed;
        d=1e100.net; s=20230601; t=1772572549; x=1773177349;
        h=cc:to:in-reply-to:references:message-id:content-transfer-encoding
         :mime-version:subject:date:from:x-gm-gg:x-gm-message-state:from:to
         :cc:subject:date:message-id:reply-to;
        bh=Nsl81xXoWOrG7fZ6D3nnOVMYFTWE73QzzhAJNIpS3Uk=;
        b=nnmTpUrCcqUHlUXYyjy0Lih95QOIguSx79SgMYX/8WAt8lE7ICY12FNPalLOCvn7JH
         jiuyTXJSGFNsYTlrd1j8HgMSBOlOlOlqbfXN0ATRQ4N7AazwqZ7Ipsj17PlplNOXE/Be
         MaPqk77kg76G0eH4LtRtLfqyp3as8QWyIMo3QKk+KexhmnHEMHR0Dl1UUxCJam5HqYu5
         0DqqjskrMfX1e/vaXtOb7GFNJl5mQaYtRw7YPV4xuzlJs8lhU9QKL1t/KDgjGDqGKSry
         8KmM+HGkEHqJxdpjPKz1uSYQuBUHVUeow46IacWBLg7KEM7NsxLvxolR8xvH24Dhdtyu
         lBNg==
X-Forwarded-Encrypted: i=1; AJvYcCUcnwkFKRHNBYTQ9J6GY9ySy7VYEdc6gxK8Rb73gt8GO1YbyYZTtNKbyCb32+fdmXLS3jAhp8QLDsOmULQjRg==@vger.kernel.org
X-Gm-Message-State: AOJu0YzNyxkw+BjFp0AsIr9q3BLiQ3UiPnr24GMv/VvcTwxB3gZwjuVi
	lEinOlvm/IBPyzPL1kvAi+fQq2C8VjbEUkZopInkuIW2NJWcj9hMCpwdCWFLqmPIwlMLGMbtd9i
	rzmhgpJoWKrIW+Va+Fhknt5AMtPnio7Miz4e1btL5bg/FnGrSj5/9Nipq5/FEv8bZHt/x
X-Gm-Gg: ATEYQzyfVorbZXT5QU/X02zfq6XjChbLWPLrf0AB49uHLTiZ73OFnoU5SMN7X7wicbM
	frGuRT5w9Kd3YMlrPqpL7ehsQ6jID8wev6nLpgMsswDc0Sj1z3O/dUrB0MjbcU+r4WfXSFlzytS
	QuL/6HfnMMCSfT9AIT7zWQdhxr0EJc3g4V6n5gO1wUkfbXaY3TiU26E1zo/bNtjD82FrRKRFT7X
	OfgcCFJcerqfRan0bBJ4tmVf4AtRF0pjdw+jloWVbhEozvU16L6D04EgW2oWSN7jH1xULr3fB4B
	GU3T8DvOmJzOiHGuejjM0DDLpp9k0iaDs1GfpLHZhcChfUmlHd4kP2Y8CJSUWDdWnX/Kb/hoAxr
	OzjNQ+/vddt2TtYsssz/n/RVFgg==
X-Received: by 2002:a05:622a:1895:b0:4f1:ccea:6744 with SMTP id d75a77b69052e-507528db6e5mr222734461cf.58.1772572549567;
        Tue, 03 Mar 2026 13:15:49 -0800 (PST)
X-Received: by 2002:a05:622a:1895:b0:4f1:ccea:6744 with SMTP id d75a77b69052e-507528db6e5mr222733911cf.58.1772572549028;
        Tue, 03 Mar 2026 13:15:49 -0800 (PST)
Received: from [172.16.1.8] ([2607:f2c0:b1e3:9a00:3c7:56c2:f819:96d2])
        by smtp.gmail.com with ESMTPSA id d75a77b69052e-5074481c0e5sm156286991cf.0.2026.03.03.13.15.46
        (version=TLS1_3 cipher=TLS_AES_256_GCM_SHA384 bits=256/256);
        Tue, 03 Mar 2026 13:15:47 -0800 (PST)
From: Peter Colberg <pcolberg@redhat.com>
Date: Tue, 03 Mar 2026 16:15:28 -0500
Subject: [PATCH v3 08/10] rust: pci: add num_vf(), to return number of VFs
Precedence: bulk
X-Mailing-List: rust-for-linux@vger.kernel.org
List-Id: <rust-for-linux.vger.kernel.org>
List-Subscribe: <mailto:rust-for-linux+subscribe@vger.kernel.org>
List-Unsubscribe: <mailto:rust-for-linux+unsubscribe@vger.kernel.org>
MIME-Version: 1.0
Message-Id: <20260303-rust-pci-sriov-v3-8-4443c35f0c88@redhat.com>
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
X-Mimecast-MFC-PROC-ID: I6u0Di9BMLH8Xc3CmnV8QX6W8oOj6VPcFpR8nS9EAs0_1772572550
X-Mimecast-Originator: redhat.com
Content-Type: text/plain; charset="utf-8"
Content-Transfer-Encoding: 7bit

Add a method to return the number of Virtual Functions (VF) enabled for
a Physical Function (PF).

Signed-off-by: Peter Colberg <pcolberg@redhat.com>
---
 rust/kernel/pci.rs | 7 +++++++
 1 file changed, 7 insertions(+)

diff --git a/rust/kernel/pci.rs b/rust/kernel/pci.rs
index df39ad3f0d5fd898b034609efb03368f83c2a2e9..581930d0afe98ccc29d729e4d9aab75b4144e46c 100644
--- a/rust/kernel/pci.rs
+++ b/rust/kernel/pci.rs
@@ -498,6 +498,13 @@ pub fn is_virtfn(&self) -> bool {
         unsafe { (*self.as_raw()).is_virtfn() != 0 }
     }
 
+    /// Returns the number of Virtual Functions (VF) enabled for a Physical Function (PF).
+    #[cfg(CONFIG_PCI_IOV)]
+    pub fn num_vf(&self) -> i32 {
+        // SAFETY: `self.as_raw` is a valid pointer to a `struct pci_dev`.
+        unsafe { bindings::pci_num_vf(self.as_raw()) }
+    }
+
     /// Returns the size of the given PCI BAR resource.
     pub fn resource_len(&self, bar: u32) -> Result<bindings::resource_size_t> {
         if !Bar::index_is_valid(bar) {

-- 
2.53.0

