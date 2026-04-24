Received: from us-smtp-delivery-124.mimecast.com (us-smtp-delivery-124.mimecast.com [170.10.133.124])
	(using TLSv1.2 with cipher ECDHE-RSA-AES256-GCM-SHA384 (256/256 bits))
	(No client certificate requested)
	by smtp.subspace.kernel.org (Postfix) with ESMTPS id 347E442F55E
	for <rust-for-linux@vger.kernel.org>; Tue,  3 Mar 2026 21:15:49 +0000 (UTC)
Authentication-Results: smtp.subspace.kernel.org; arc=none smtp.client-ip=170.10.133.124
ARC-Seal:i=1; a=rsa-sha256; d=subspace.kernel.org; s=arc-20240116;
	t=1772572550; cv=none; b=m3hu+oU0TKhF6OVkypX+BIU9hKOXgxObcpXTzMEMXB1FXxnGD+zXjE/4lv2YIgesvtUyX1aQfQwCBzN2SjTvwH/B0Dgzy+c4DZ9yTe9TYMx7uw6mkueg5FczpW245U/RvFEGmgiYIxKcS21Rr84hQnSCG2WjjYLrkUtigsRhYUI=
ARC-Message-Signature:i=1; a=rsa-sha256; d=subspace.kernel.org;
	s=arc-20240116; t=1772572550; c=relaxed/simple;
	bh=s6mYU8Hu6u0tbKnwvAlOQsSAlR2qHgbV7fvzmNOhT1I=;
	h=From:Date:Subject:MIME-Version:Message-Id:References:In-Reply-To:
	 To:Cc:Content-Type; b=CZfha/ThNJ8XCd59J8b7SiN8SETFrJggPwvZaElb06ZrVQB6QVNmXvv0PFQNiuZ4qlTEoJ24k6yt6ckk8fFWS56BlKOUxS4hVsp09GYJ3zwBWrFawoE5Vt/rtUolN3MBb6WnxR1LSezDqHZxaDAad0OmDzzo8ePhycA2LSMQzL4=
ARC-Authentication-Results:i=1; smtp.subspace.kernel.org; dmarc=pass (p=quarantine dis=none) header.from=redhat.com; spf=pass smtp.mailfrom=redhat.com; dkim=pass (1024-bit key) header.d=redhat.com header.i=@redhat.com header.b=BTkyQ1MI; arc=none smtp.client-ip=170.10.133.124
Authentication-Results: smtp.subspace.kernel.org; dmarc=pass (p=quarantine dis=none) header.from=redhat.com
Authentication-Results: smtp.subspace.kernel.org; spf=pass smtp.mailfrom=redhat.com
Authentication-Results: smtp.subspace.kernel.org;
	dkim=pass (1024-bit key) header.d=redhat.com header.i=@redhat.com header.b="BTkyQ1MI"
DKIM-Signature: v=1; a=rsa-sha256; c=relaxed/relaxed; d=redhat.com;
	s=mimecast20190719; t=1772572548;
	h=from:from:reply-to:subject:subject:date:date:message-id:message-id:
	 to:to:cc:cc:mime-version:mime-version:content-type:content-type:
	 content-transfer-encoding:content-transfer-encoding:
	 in-reply-to:in-reply-to:references:references;
	bh=qLYDkNQ8A4x2T95jGRETa6NwStvleq7OKd2uXV0vocw=;
	b=BTkyQ1MIwdUr4SBnXaUoXkx2/qFXyrU5CIbfGcjArGrolbt/qJhAi2ziLVJz6vGm6IY23v
	UryGmw1nwaJVbxhzmYw/2pymwQ8XtHp17lugdIOZ9Q9g6Og820zug/p1Ip5J8SE0Mk91mb
	7FMRsKvQOEObo489Ivyt+Wyxi/oCehw=
Received: from mail-qt1-f199.google.com (mail-qt1-f199.google.com
 [209.85.160.199]) by relay.mimecast.com with ESMTP with STARTTLS
 (version=TLSv1.3, cipher=TLS_AES_256_GCM_SHA384) id
 us-mta-2-eXBQ_MbgNXeN4E7HbRPsjQ-1; Tue, 03 Mar 2026 16:15:47 -0500
X-MC-Unique: eXBQ_MbgNXeN4E7HbRPsjQ-1
X-Mimecast-MFC-AGG-ID: eXBQ_MbgNXeN4E7HbRPsjQ_1772572547
Received: by mail-qt1-f199.google.com with SMTP id d75a77b69052e-5033c483b74so85775941cf.2
        for <rust-for-linux@vger.kernel.org>; Tue, 03 Mar 2026 13:15:47 -0800 (PST)
X-Google-DKIM-Signature: v=1; a=rsa-sha256; c=relaxed/relaxed;
        d=1e100.net; s=20230601; t=1772572547; x=1773177347;
        h=cc:to:in-reply-to:references:message-id:content-transfer-encoding
         :mime-version:subject:date:from:x-gm-gg:x-gm-message-state:from:to
         :cc:subject:date:message-id:reply-to;
        bh=qLYDkNQ8A4x2T95jGRETa6NwStvleq7OKd2uXV0vocw=;
        b=xH/rr8Io8jDkpTVm/nKWwg0wE/WDhztx5gFNyZRFjmyB0CmhUX/AWJ47lnwaRbzddG
         h35MHOZn97PP7d767CMC/TzCXEZcCyCYcpsMtZFlnSm5woNhoxjOO1xKEADItP9M7qQM
         BBElMkQfg8VgJ09Cq+tSEFsV3cxL1IR2ElUwWRG41tZfS2onyuops28UprVoEWwulsmR
         TJz+c0RhrwXgXFM+2lfjqASjzGNi0GZpUI5q0fHbnpBeauiBhUtaolvF0QDWwm80ALSO
         WZ7rBxQzufOyPHO0pxQzIWCa5TFWMbA6Fb1dql2b+mEFAPa2XNOEI/xCcSHNl+OKUk5B
         8cBA==
X-Forwarded-Encrypted: i=1; AJvYcCWxn0vN/Aopm0AbvA4IgWHWGZMcwjJPXZDr9qFT3MW/CzzbR/wQr+HdnNk7tI31DM948tJ7J4AsLZXoVfjdsw==@vger.kernel.org
X-Gm-Message-State: AOJu0YzJ4KXSyrEFE/8aJ30TugnvSlCXPss85j2nU0uOmkqcJicUBTV+
	CDn5ELGu/SyEquOaXVE6oYcDtAkoyxc+gl71KhSVssSbHjtv95/216ia4MK6GBv8vj7WHq7uKde
	fBL1wmRETURYqbd8jl1cxLmL4wHkF3queQtf65/jinzPyQ+Aola/KVlMSnZhcI3ru9zPd
X-Gm-Gg: ATEYQzz4r4J76WhjXsmgkm5n61P1qgtNF3+IQ6UC21jNHtfi0nJowvQbzJMKvcxSNB3
	bPbZcM2iHexe2LncXTWTgSK3Pt94vzW5BGsAlhmGY4MgIse83R/oqiwn0P048nVVuj4TqP6Mgr5
	nbK5cU9QN4GTBvLxBCNU6nFXfwjIcHc9JYxPCxScuYtNZ1018W70WLb5Uaz7KJp9gAZ9pemDAbZ
	MsqjE3bGcxf8xApS0rwc5+bZCqKPSwRiolABPRYJg2Rd1SrJjZsp4vX4Mea0cW8/JH9BZZf2OCq
	aJ7v3O+5gan+jUuGr6HDuODQiYpmFdg2p+vWQ86pppbhYHYtDZALnXfvPGY1HtLt+VQGMg+OQwP
	2eSNCEVHBBzpnL1ERUI4rJ5FB4w==
X-Received: by 2002:a05:622a:64c:b0:4f3:5652:6743 with SMTP id d75a77b69052e-507527cc1c1mr213308581cf.39.1772572546753;
        Tue, 03 Mar 2026 13:15:46 -0800 (PST)
X-Received: by 2002:a05:622a:64c:b0:4f3:5652:6743 with SMTP id d75a77b69052e-507527cc1c1mr213308331cf.39.1772572546254;
        Tue, 03 Mar 2026 13:15:46 -0800 (PST)
Received: from [172.16.1.8] ([2607:f2c0:b1e3:9a00:3c7:56c2:f819:96d2])
        by smtp.gmail.com with ESMTPSA id d75a77b69052e-5074481c0e5sm156286991cf.0.2026.03.03.13.15.44
        (version=TLS1_3 cipher=TLS_AES_256_GCM_SHA384 bits=256/256);
        Tue, 03 Mar 2026 13:15:45 -0800 (PST)
From: Peter Colberg <pcolberg@redhat.com>
Date: Tue, 03 Mar 2026 16:15:27 -0500
Subject: [PATCH v3 07/10] rust: pci: add is_physfn(), to check for PFs
Precedence: bulk
X-Mailing-List: rust-for-linux@vger.kernel.org
List-Id: <rust-for-linux.vger.kernel.org>
List-Subscribe: <mailto:rust-for-linux+subscribe@vger.kernel.org>
List-Unsubscribe: <mailto:rust-for-linux+unsubscribe@vger.kernel.org>
MIME-Version: 1.0
Message-Id: <20260303-rust-pci-sriov-v3-7-4443c35f0c88@redhat.com>
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
X-Mimecast-MFC-PROC-ID: q8ENEcVpi8z-lUTSYroQ3VFveonCFd3QP5ZrG6vCRGg_1772572547
X-Mimecast-Originator: redhat.com
Content-Type: text/plain; charset="utf-8"
Content-Transfer-Encoding: 7bit

Add a method to check if a PCI device is a Physical Function (PF).

Reviewed-by: Joel Fernandes <joelagnelf@nvidia.com>
Signed-off-by: Peter Colberg <pcolberg@redhat.com>
---
Changes in v2:
- Replace VF -> PF in doc comment of is_physfn().
- Add #[inline] to is_physfn().
---
 rust/kernel/pci.rs | 7 +++++++
 1 file changed, 7 insertions(+)

diff --git a/rust/kernel/pci.rs b/rust/kernel/pci.rs
index db05641186c3a42922e2b6a463de9c1b099a4673..df39ad3f0d5fd898b034609efb03368f83c2a2e9 100644
--- a/rust/kernel/pci.rs
+++ b/rust/kernel/pci.rs
@@ -484,6 +484,13 @@ pub fn resource_start(&self, bar: u32) -> Result<bindings::resource_size_t> {
         Ok(unsafe { bindings::pci_resource_start(self.as_raw(), bar.try_into()?) })
     }
 
+    /// Returns `true` if this device is a Physical Function (PF).
+    #[inline]
+    pub fn is_physfn(&self) -> bool {
+        // SAFETY: `self.as_raw` is a valid pointer to a `struct pci_dev`.
+        unsafe { (*self.as_raw()).is_physfn() != 0 }
+    }
+
     /// Returns `true` if this device is a Virtual Function (VF).
     #[inline]
     pub fn is_virtfn(&self) -> bool {

-- 
2.53.0

