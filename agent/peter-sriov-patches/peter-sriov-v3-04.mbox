Received: from us-smtp-delivery-124.mimecast.com (us-smtp-delivery-124.mimecast.com [170.10.129.124])
	(using TLSv1.2 with cipher ECDHE-RSA-AES256-GCM-SHA384 (256/256 bits))
	(No client certificate requested)
	by smtp.subspace.kernel.org (Postfix) with ESMTPS id 7BD413CD8AB
	for <rust-for-linux@vger.kernel.org>; Tue,  3 Mar 2026 21:15:45 +0000 (UTC)
Authentication-Results: smtp.subspace.kernel.org; arc=none smtp.client-ip=170.10.129.124
ARC-Seal:i=1; a=rsa-sha256; d=subspace.kernel.org; s=arc-20240116;
	t=1772572547; cv=none; b=bl3/jJh6Esqqei9LD9nFWmfjYyWwodtCAf1uB81CRqgWS54WQnCb+++q4dJz6CofEKYeKM/4zZoTVaZbSL/6yJsJJFWB278WNC1X7MsN2y86L5+9ST0dWYrGwg+fGifhbIWzdALDTPNcap4w21upZ4OsSuIoNgFIwGHKEPFx/ZQ=
ARC-Message-Signature:i=1; a=rsa-sha256; d=subspace.kernel.org;
	s=arc-20240116; t=1772572547; c=relaxed/simple;
	bh=Ovi7q4LBinaNMgbwhfXjFRVkgoBkC//IcdGA6/LR988=;
	h=From:Date:Subject:MIME-Version:Message-Id:References:In-Reply-To:
	 To:Cc:Content-Type; b=l8MqSbGXuSHY45qKd800PyV49mxAvoPU1ZFbsOx5kx+yNIGR7J+NCv2/weztqEVe+uvvhRlWDQmDuGb55Ur6sHYMoHcGxfNq9pXALAn8wFqgp/NECE+46zmtBxudqPPvuKf4rP9WI30Bfle6IcxYx6TvEtimBFl8KK/bg+yHZ40=
ARC-Authentication-Results:i=1; smtp.subspace.kernel.org; dmarc=pass (p=quarantine dis=none) header.from=redhat.com; spf=pass smtp.mailfrom=redhat.com; dkim=pass (1024-bit key) header.d=redhat.com header.i=@redhat.com header.b=X+snYV5Y; arc=none smtp.client-ip=170.10.129.124
Authentication-Results: smtp.subspace.kernel.org; dmarc=pass (p=quarantine dis=none) header.from=redhat.com
Authentication-Results: smtp.subspace.kernel.org; spf=pass smtp.mailfrom=redhat.com
Authentication-Results: smtp.subspace.kernel.org;
	dkim=pass (1024-bit key) header.d=redhat.com header.i=@redhat.com header.b="X+snYV5Y"
DKIM-Signature: v=1; a=rsa-sha256; c=relaxed/relaxed; d=redhat.com;
	s=mimecast20190719; t=1772572544;
	h=from:from:reply-to:subject:subject:date:date:message-id:message-id:
	 to:to:cc:cc:mime-version:mime-version:content-type:content-type:
	 content-transfer-encoding:content-transfer-encoding:
	 in-reply-to:in-reply-to:references:references;
	bh=nmGZI5WblWd631ssrKt08f4BijC6XK/3q+5rIjFI99c=;
	b=X+snYV5YDAJDuDwdhgZsnhFnZfstNwyY7PJzJbTCf7i7oUREYurhBsZZqXs8TFjt4XQ3SG
	pzrkOQ+/uCrutTgiEuBkfK2le0SVVWkltR9l5zrtPz5dL9LluFfjv5g+JqH4Ha7T2k+GtM
	9C1mzkHdotWTzvFg6AHomt7t9+MJJaU=
Received: from mail-qt1-f198.google.com (mail-qt1-f198.google.com
 [209.85.160.198]) by relay.mimecast.com with ESMTP with STARTTLS
 (version=TLSv1.3, cipher=TLS_AES_256_GCM_SHA384) id
 us-mta-133-GURKlVvyN2KQKTHVCJ1Auw-1; Tue, 03 Mar 2026 16:15:42 -0500
X-MC-Unique: GURKlVvyN2KQKTHVCJ1Auw-1
X-Mimecast-MFC-AGG-ID: GURKlVvyN2KQKTHVCJ1Auw_1772572542
Received: by mail-qt1-f198.google.com with SMTP id d75a77b69052e-506bac14430so716535651cf.2
        for <rust-for-linux@vger.kernel.org>; Tue, 03 Mar 2026 13:15:42 -0800 (PST)
X-Google-DKIM-Signature: v=1; a=rsa-sha256; c=relaxed/relaxed;
        d=1e100.net; s=20230601; t=1772572542; x=1773177342;
        h=cc:to:in-reply-to:references:message-id:content-transfer-encoding
         :mime-version:subject:date:from:x-gm-gg:x-gm-message-state:from:to
         :cc:subject:date:message-id:reply-to;
        bh=nmGZI5WblWd631ssrKt08f4BijC6XK/3q+5rIjFI99c=;
        b=uLJZEdlnI9xn1FgY0a+UpIVi7mW8Sja9Un/nzJ0c80zik+p7l2E8el0MquT5z2ZIAY
         AQvMeuzypE1J7BsSc7b5BYURBqhKh5KHGNdoff9R0R0ylBVEIkvrOLWe/xuD4IikfJTj
         CApScpY7jHIIkI7VebUyPU8lxGG/AE1/rPL4FpWT6KS+ODruWmx2vTtOjy6bPziNWUQa
         fH6JYrKjZd792thCDoAwmVFmfSBxaR98kLDndgwYZNp7FJb4ElgSFykdWcSHGN6RH5c3
         OO+TrINEpe6e5HO/72Dn66Aje2G4e4J4uWsAQ0+FK8/6yjMCFIW91a+JklPwRtbX0oDe
         tGuw==
X-Forwarded-Encrypted: i=1; AJvYcCW7nkBXmpWNPz1R3Mof8SRywasuPhqrjCVeIEia9yv5+D5/mGgDmaQETxZZx7Q27f0aZbVhDoLHCfT3KWjMxw==@vger.kernel.org
X-Gm-Message-State: AOJu0Yx4Kua7WWRMABE0B8XW6yKcvnui8IAudkbq6L/H6bi9C6OYel8D
	NDGLCb2mZ0u1GoL9pOYaaNPHpXfzNfGaIRDk39R4b2lBXxE7wtqhC0AVo4lVHiyN2kVrNXytxId
	gjqbtIwO3txpRd3M1fLnNhHGxprDonHjKjQUYNZghpGEHzimftaPPgf5jLXSz7KHdrcgY
X-Gm-Gg: ATEYQzwCAAsEv4vEmJ2eGyB/s80otBk5s3Biz6P2WvpuO986d1POlbmJANUNXB7nZnv
	2YiLoeyDFaVDqoQTy0sZGnI6a31nEWsfKxjXpYrEsfFioZwypE4nDgatT6/NZSUfu3YuIITaOhZ
	gKTD1q4wquVB4ksTOOc0cyQyh0rIaWzTN/n2KMCRKiX8PtsMR4EObE2DFZ+dKyBkZyOQRBlTXSs
	tH39AvRks9MAt8BFnRyuWkfZvqfuOdKqW2HF5aW6g+RdkTi1ERRf2tIPq/k/d5cNNJEB63P8MgQ
	R676fzQ7WtbRt+kV13Up1om1oWvGTQnVI0TU7qbhbKdc0Ykc3qjpk2cRnJmdBqgTB/9TV4UI3ij
	8iH1qPpbXJc96rk56cHwEKWMncQ==
X-Received: by 2002:a05:622a:18a8:b0:502:a1bc:5691 with SMTP id d75a77b69052e-507525a5444mr232050351cf.0.1772572541799;
        Tue, 03 Mar 2026 13:15:41 -0800 (PST)
X-Received: by 2002:a05:622a:18a8:b0:502:a1bc:5691 with SMTP id d75a77b69052e-507525a5444mr232049271cf.0.1772572540438;
        Tue, 03 Mar 2026 13:15:40 -0800 (PST)
Received: from [172.16.1.8] ([2607:f2c0:b1e3:9a00:3c7:56c2:f819:96d2])
        by smtp.gmail.com with ESMTPSA id d75a77b69052e-5074481c0e5sm156286991cf.0.2026.03.03.13.15.38
        (version=TLS1_3 cipher=TLS_AES_256_GCM_SHA384 bits=256/256);
        Tue, 03 Mar 2026 13:15:40 -0800 (PST)
From: Peter Colberg <pcolberg@redhat.com>
Date: Tue, 03 Mar 2026 16:15:24 -0500
Subject: [PATCH v3 04/10] rust: pci: add vtable attribute to pci::Driver
 trait
Precedence: bulk
X-Mailing-List: rust-for-linux@vger.kernel.org
List-Id: <rust-for-linux.vger.kernel.org>
List-Subscribe: <mailto:rust-for-linux+subscribe@vger.kernel.org>
List-Unsubscribe: <mailto:rust-for-linux+unsubscribe@vger.kernel.org>
MIME-Version: 1.0
Message-Id: <20260303-rust-pci-sriov-v3-4-4443c35f0c88@redhat.com>
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
X-Mimecast-MFC-PROC-ID: up25oWxnQCA851nzvV7zitXWDpOrTGKfZ0Kv8lXLWcY_1772572542
X-Mimecast-Originator: redhat.com
Content-Type: text/plain; charset="utf-8"
Content-Transfer-Encoding: 7bit

Add the #[vtable] attribute to pci::Driver trait and implementations,
to prepare a subsequent patch that adds an optional bus callback
sriov_configure() to enable or disable the SR-IOV capability.

Suggested-by: Danilo Krummrich <dakr@kernel.org>
Signed-off-by: Peter Colberg <pcolberg@redhat.com>
---
Changes in v2:
- Add missing #[vtable] attribute in PCI driver trait example.
- Add missing #[vtable] attribute in nova-core driver.
---
 drivers/gpu/nova-core/driver.rs       | 1 +
 rust/kernel/pci.rs                    | 2 ++
 samples/rust/rust_dma.rs              | 1 +
 samples/rust/rust_driver_auxiliary.rs | 1 +
 samples/rust/rust_driver_pci.rs       | 1 +
 5 files changed, 6 insertions(+)

diff --git a/drivers/gpu/nova-core/driver.rs b/drivers/gpu/nova-core/driver.rs
index 5a4cc047bcfc9fcef61373ace84ed43958a3bcbd..66a68048006fb33477bb00f61856c1aa92c0a8f1 100644
--- a/drivers/gpu/nova-core/driver.rs
+++ b/drivers/gpu/nova-core/driver.rs
@@ -64,6 +64,7 @@ pub(crate) struct NovaCore {
     ]
 );
 
+#[vtable]
 impl pci::Driver for NovaCore {
     type IdInfo = ();
     const ID_TABLE: pci::IdTable<Self::IdInfo> = &PCI_TABLE;
diff --git a/rust/kernel/pci.rs b/rust/kernel/pci.rs
index e1cab1574a3d309d25bf5267c0b0d8da8fb66d44..a4c27c674bd8bdf5e3316789d38d566e90b93fe2 100644
--- a/rust/kernel/pci.rs
+++ b/rust/kernel/pci.rs
@@ -278,6 +278,7 @@ macro_rules! pci_device_table {
 ///     ]
 /// );
 ///
+/// #[vtable]
 /// impl pci::Driver for MyDriver {
 ///     type IdInfo = ();
 ///     const ID_TABLE: pci::IdTable<Self::IdInfo> = &PCI_TABLE;
@@ -292,6 +293,7 @@ macro_rules! pci_device_table {
 ///```
 /// Drivers must implement this trait in order to get a PCI driver registered. Please refer to the
 /// `Adapter` documentation for an example.
+#[vtable]
 pub trait Driver: Send {
     /// The type holding information about each device id supported by the driver.
     // TODO: Use `associated_type_defaults` once stabilized:
diff --git a/samples/rust/rust_dma.rs b/samples/rust/rust_dma.rs
index 9c45851c876ef33414eb0071c42a2fb4ac3f1e78..ae6f7328b830e32bcaf7f8b5f8b1f117135ebf8e 100644
--- a/samples/rust/rust_dma.rs
+++ b/samples/rust/rust_dma.rs
@@ -51,6 +51,7 @@ unsafe impl kernel::transmute::FromBytes for MyStruct {}
     [(pci::DeviceId::from_id(pci::Vendor::REDHAT, 0x5), ())]
 );
 
+#[vtable]
 impl pci::Driver for DmaSampleDriver {
     type IdInfo = ();
     const ID_TABLE: pci::IdTable<Self::IdInfo> = &PCI_TABLE;
diff --git a/samples/rust/rust_driver_auxiliary.rs b/samples/rust/rust_driver_auxiliary.rs
index 5c5a5105a3fff90f5e396186776e1b3ffdf479b4..2f7ac8f7391f45827c086b704950fd01907c1825 100644
--- a/samples/rust/rust_driver_auxiliary.rs
+++ b/samples/rust/rust_driver_auxiliary.rs
@@ -65,6 +65,7 @@ struct ParentDriver {
     [(pci::DeviceId::from_id(pci::Vendor::REDHAT, 0x5), ())]
 );
 
+#[vtable]
 impl pci::Driver for ParentDriver {
     type IdInfo = ();
 
diff --git a/samples/rust/rust_driver_pci.rs b/samples/rust/rust_driver_pci.rs
index d3d4a7931deb0c1085cfd817990737717f466ea9..27e603a9509c19b6845c10ef06a0af897aa0e84b 100644
--- a/samples/rust/rust_driver_pci.rs
+++ b/samples/rust/rust_driver_pci.rs
@@ -92,6 +92,7 @@ fn config_space(pdev: &pci::Device<Bound>) {
     }
 }
 
+#[vtable]
 impl pci::Driver for SampleDriver {
     type IdInfo = TestIndex;
 

-- 
2.53.0

