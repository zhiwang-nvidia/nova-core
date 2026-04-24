Received: from us-smtp-delivery-124.mimecast.com (us-smtp-delivery-124.mimecast.com [170.10.129.124])
	(using TLSv1.2 with cipher ECDHE-RSA-AES256-GCM-SHA384 (256/256 bits))
	(No client certificate requested)
	by smtp.subspace.kernel.org (Postfix) with ESMTPS id D337F3CB2E4
	for <rust-for-linux@vger.kernel.org>; Tue,  3 Mar 2026 21:15:36 +0000 (UTC)
Authentication-Results: smtp.subspace.kernel.org; arc=none smtp.client-ip=170.10.129.124
ARC-Seal:i=1; a=rsa-sha256; d=subspace.kernel.org; s=arc-20240116;
	t=1772572538; cv=none; b=aZUoyJ2ZertxNRBWwCFkByiizQk024Q/1GOVSuIoC374ltzB3J7c5/pWk3EqE98nEbTIyO8vlv6E4QpZ6G36/vt4crzLhJlTAkbp/NuFns5sc+leZyBifzTwSX/V3E7s97Xt5Bx3ygWKtYc0p7OzbrEOwX/WuCL5knTd+GIqla8=
ARC-Message-Signature:i=1; a=rsa-sha256; d=subspace.kernel.org;
	s=arc-20240116; t=1772572538; c=relaxed/simple;
	bh=ax5r1OmCHwEBpaIwBxT5kM3++FSx0BGrf1GkfVvAo60=;
	h=From:Date:Subject:MIME-Version:Message-Id:References:In-Reply-To:
	 To:Cc:Content-Type; b=MRdjas/BJ7rsc84RBlcdJkybYICQwNH+4TwSuWNLcI14r+GftkgXwaFDJwh8qpjUSP/YOrLcV6rAsgjrpSE3F6PYhaQuF0/UONJ1nIBENKKFDKMgR5b/+H6JWRyUBs/J8gKpO3yRCC2S2M/bx0ieB2rWpIF6IA8koTl75TPBPbY=
ARC-Authentication-Results:i=1; smtp.subspace.kernel.org; dmarc=pass (p=quarantine dis=none) header.from=redhat.com; spf=pass smtp.mailfrom=redhat.com; dkim=pass (1024-bit key) header.d=redhat.com header.i=@redhat.com header.b=ZQnk34Oe; arc=none smtp.client-ip=170.10.129.124
Authentication-Results: smtp.subspace.kernel.org; dmarc=pass (p=quarantine dis=none) header.from=redhat.com
Authentication-Results: smtp.subspace.kernel.org; spf=pass smtp.mailfrom=redhat.com
Authentication-Results: smtp.subspace.kernel.org;
	dkim=pass (1024-bit key) header.d=redhat.com header.i=@redhat.com header.b="ZQnk34Oe"
DKIM-Signature: v=1; a=rsa-sha256; c=relaxed/relaxed; d=redhat.com;
	s=mimecast20190719; t=1772572536;
	h=from:from:reply-to:subject:subject:date:date:message-id:message-id:
	 to:to:cc:cc:mime-version:mime-version:content-type:content-type:
	 content-transfer-encoding:content-transfer-encoding:
	 in-reply-to:in-reply-to:references:references;
	bh=0eBeiZqE9TXbxZv4w/1cPbOmShRF2lMY9/B+OMDvXLw=;
	b=ZQnk34Oec2sKe1yWNMaiu969Ky+LleJ+jxvykN1He/Tb/+vEikxfudKai7z/0XIAtre+z6
	G+AtPvkxFMJunBaMKMkSr2yr8xsKCjHJbbBrJdkmAmLsSzH1xbw12oXqYvgJY0BoQYityZ
	0CrmRWsR6OuhnJ6e9Hn1qy8/B+FnsIw=
Received: from mail-qt1-f199.google.com (mail-qt1-f199.google.com
 [209.85.160.199]) by relay.mimecast.com with ESMTP with STARTTLS
 (version=TLSv1.3, cipher=TLS_AES_256_GCM_SHA384) id
 us-mta-343-dAp8ad_4MCWUjGDSLT_CdQ-1; Tue, 03 Mar 2026 16:15:34 -0500
X-MC-Unique: dAp8ad_4MCWUjGDSLT_CdQ-1
X-Mimecast-MFC-AGG-ID: dAp8ad_4MCWUjGDSLT_CdQ_1772572534
Received: by mail-qt1-f199.google.com with SMTP id d75a77b69052e-504888a2a1dso454788111cf.0
        for <rust-for-linux@vger.kernel.org>; Tue, 03 Mar 2026 13:15:34 -0800 (PST)
X-Google-DKIM-Signature: v=1; a=rsa-sha256; c=relaxed/relaxed;
        d=1e100.net; s=20230601; t=1772572534; x=1773177334;
        h=cc:to:in-reply-to:references:message-id:content-transfer-encoding
         :mime-version:subject:date:from:x-gm-gg:x-gm-message-state:from:to
         :cc:subject:date:message-id:reply-to;
        bh=0eBeiZqE9TXbxZv4w/1cPbOmShRF2lMY9/B+OMDvXLw=;
        b=c5foeCi6vWHPU5d/Ilm+P7C0g2JPejUQnPudJgW8CkJdCJEGYWSrjuRa+Bbf1AKZgD
         5RdRJcl9JkUI1mbMajnonms+e0TlS7Op88/LJT9COK++xHNdPi6U1+gHiF+PTiPcHvCJ
         Otfy5bv3fo3yN2Yqv5sC/X+Wcbb6mn72vnlFkuGPNdS685YdMMcBNDUt2CPJ0NHjrAYD
         1kATv/prQJzPszdX7UFTltuwgQY/V8DKDXeDHOWtJcZqvaWIV7iMpkXA2VkHUMuk7242
         0rNjqNaQDE4qjucQsOpWqYgegTqCmaCr+cUI0LiCIiOQpSELfzC/Y5nc0Sn9jKJVYJNm
         qw6w==
X-Forwarded-Encrypted: i=1; AJvYcCW5YEtc8aoWI/LqEJeOU0h01BdKFbX6QO++htLiDlpIYGdwT8vo0M7940JU/GR9p7ZB32sbk+8v/obz+XcyhA==@vger.kernel.org
X-Gm-Message-State: AOJu0YznPeYX/Gm/c/pNiZ1Wc/RYqRKWiOL+2Ohr9pXW8hEAqCjDN/dV
	v+3ot/3uKkPA+bRhpxkHHTX76gnodNrfxvu9GShJq5cdO/dIcGBGRaIEi/xDPxiM8z3Vv7oIEfd
	hhXKsJxxeGBxGq2DlUhrfJBckMT4ElGsLpSa/UDAyNocdOOOl9zjbhLh4iFEL8skWmZQJ
X-Gm-Gg: ATEYQzyuLEJjS1f2FYAmbcmVkz+aUVveImcooig4u9qnG+h9TCDPn1qg4uYJ+4vhS2v
	dOXjQB/IyhgYnG4wFNXCg8Y1dIrVYOnsqzkochI3mc8mp9Hw0GSEDm2x8/extT76YIfy6R1N/Qr
	T7gyaiZjrJ/lUk92flAj8E3gSexCRn3Fj376Sk9IZY9olWZSr0dX2VFCOdbFT6FXmEn5A5uN+qe
	tNpqoXtuMcVrg5HMZyqB+KAX6a05bCwWhUpShFmdE0Kfj59AbOLoEN/bH7Td6/ViSFw4IFjabbL
	JAMlIRhlW31QcwUj6ondr6hAf1yiSSGl1l2dQ9/6j0bLXiNU2UD2tIhIwjFS9ZCo8AEcDH4Liac
	j00Uoi9BjGO8nChh00ilGoy1isQ==
X-Received: by 2002:ac8:5f8f:0:b0:506:a1a8:c6fb with SMTP id d75a77b69052e-507528765a2mr213886841cf.2.1772572534184;
        Tue, 03 Mar 2026 13:15:34 -0800 (PST)
X-Received: by 2002:ac8:5f8f:0:b0:506:a1a8:c6fb with SMTP id d75a77b69052e-507528765a2mr213886271cf.2.1772572533578;
        Tue, 03 Mar 2026 13:15:33 -0800 (PST)
Received: from [172.16.1.8] ([2607:f2c0:b1e3:9a00:3c7:56c2:f819:96d2])
        by smtp.gmail.com with ESMTPSA id d75a77b69052e-5074481c0e5sm156286991cf.0.2026.03.03.13.15.31
        (version=TLS1_3 cipher=TLS_AES_256_GCM_SHA384 bits=256/256);
        Tue, 03 Mar 2026 13:15:33 -0800 (PST)
From: Peter Colberg <pcolberg@redhat.com>
Date: Tue, 03 Mar 2026 16:15:21 -0500
Subject: [PATCH v3 01/10] PCI: add driver flag to opt into disabling SR-IOV
 on remove()
Precedence: bulk
X-Mailing-List: rust-for-linux@vger.kernel.org
List-Id: <rust-for-linux.vger.kernel.org>
List-Subscribe: <mailto:rust-for-linux+subscribe@vger.kernel.org>
List-Unsubscribe: <mailto:rust-for-linux+unsubscribe@vger.kernel.org>
MIME-Version: 1.0
Message-Id: <20260303-rust-pci-sriov-v3-1-4443c35f0c88@redhat.com>
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
X-Mimecast-MFC-PROC-ID: zZo_C5aHTBIbEs_xJt2W0WmYzuJv7wXv-X-CDkVVKhs_1772572534
X-Mimecast-Originator: redhat.com
Content-Type: text/plain; charset="utf-8"
Content-Transfer-Encoding: 7bit

Add a flag managed_sriov to the pci_driver structure that allows a
driver to opt into disabling the Single Root I/O Virtualization (SR-IOV)
capability of the device when the driver is unbound.

Add a new function pci_iov_disable() that is invoked before the remove()
callback of a PCI driver and checks for the presence of the new flag.
If the flag is set, invoke the sriov_configure() callback to allow the
driver to gracefully disable SR-IOV. Warn if the driver fails to do so
and forcibly disable SR-IOV using sriov_disable().

Since a (broken) driver may theoretically re-enable SR-IOV during its
remove() callback, extend pci_iov_remove() to forcibly disable SR-IOV
after remove() if needed and only if the flag managed_sriov is set.

Altogether the flag ensures that when a Virtual Function (VF) is bound
to a driver, the corresponding Physical Function (PF) is bound to a
driver, too, since the VF devices are destroyed when the PF driver is
unbound. This guarantee is a prerequisite for exposing a safe Rust
API that allows a VF driver to obtain the PF device for a VF device
and subsequently access the device private data of the PF device.

Suggested-by: Danilo Krummrich <dakr@kernel.org>
Signed-off-by: Peter Colberg <pcolberg@redhat.com>
---
Changes in v2:
- Move logic to disable SR-IOV on remove() from Rust to C.
- Add driver flag managed_sriov to opt into disabling SR-IOV on remove().
---
 drivers/pci/iov.c        | 41 ++++++++++++++++++++++++++++++++++++++++-
 drivers/pci/pci-driver.c |  3 ++-
 drivers/pci/pci.h        |  2 ++
 include/linux/pci.h      |  8 ++++++++
 4 files changed, 52 insertions(+), 2 deletions(-)

diff --git a/drivers/pci/iov.c b/drivers/pci/iov.c
index 91ac4e37ecb9c0c5265aa40c235e84b430f43a96..da64d6ce5d30de8a52089b36fcb013937cf8b6fe 100644
--- a/drivers/pci/iov.c
+++ b/drivers/pci/iov.c
@@ -1010,20 +1010,59 @@ void pci_iov_release(struct pci_dev *dev)
 		sriov_release(dev);
 }
 
+/**
+ * pci_iov_disable - disable SR-IOV before PF driver is detached
+ * @dev: the PCI device
+ *
+ * Invoke sriov_configure() callback to allow the driver to gracefully disable
+ * SR-IOV. Warn if the driver fails to do so and forcibly disable SR-IOV.
+ */
+void pci_iov_disable(struct pci_dev *dev)
+{
+	struct pci_driver *drv = dev->driver;
+	struct pci_sriov *iov = dev->sriov;
+
+	if (WARN_ON(!drv))
+		return;
+
+	if (!dev->is_physfn || !iov->num_VFs || !drv->managed_sriov)
+		return;
+
+	if (!drv->sriov_configure) {
+		sriov_disable(dev);
+		return;
+	}
+
+	drv->sriov_configure(dev, 0);
+
+	if (WARN_ON(iov->num_VFs))
+		sriov_disable(dev);
+}
+
 /**
  * pci_iov_remove - clean up SR-IOV state after PF driver is detached
  * @dev: the PCI device
  */
 void pci_iov_remove(struct pci_dev *dev)
 {
+	struct pci_driver *drv = dev->driver;
 	struct pci_sriov *iov = dev->sriov;
 
+	if (WARN_ON(!drv))
+		return;
+
 	if (!dev->is_physfn)
 		return;
 
 	iov->driver_max_VFs = iov->total_VFs;
-	if (iov->num_VFs)
+
+	if (iov->num_VFs && !drv->managed_sriov) {
 		pci_warn(dev, "driver left SR-IOV enabled after remove\n");
+		return;
+	}
+
+	if (WARN_ON(iov->num_VFs))
+		sriov_disable(dev);
 }
 
 /**
diff --git a/drivers/pci/pci-driver.c b/drivers/pci/pci-driver.c
index dd9075403987d84e068014b35745e8872e93fdae..3fe43711565a3eb61a06cc3700e5ca953961fbe9 100644
--- a/drivers/pci/pci-driver.c
+++ b/drivers/pci/pci-driver.c
@@ -491,6 +491,7 @@ static void pci_device_remove(struct device *dev)
 	struct pci_dev *pci_dev = to_pci_dev(dev);
 	struct pci_driver *drv = pci_dev->driver;
 
+	pci_iov_disable(pci_dev);
 	if (drv->remove) {
 		pm_runtime_get_sync(dev);
 		/*
@@ -504,8 +505,8 @@ static void pci_device_remove(struct device *dev)
 		pm_runtime_put_noidle(dev);
 	}
 	pcibios_free_irq(pci_dev);
-	pci_dev->driver = NULL;
 	pci_iov_remove(pci_dev);
+	pci_dev->driver = NULL;
 
 	/* Undo the runtime PM settings in local_pci_probe() */
 	pm_runtime_put_sync(dev);
diff --git a/drivers/pci/pci.h b/drivers/pci/pci.h
index 13d998fbacce6698514d92500dfea03cc562cdc2..66308f5126ff9e4bebb537a541f1dd8717bccbfa 100644
--- a/drivers/pci/pci.h
+++ b/drivers/pci/pci.h
@@ -943,6 +943,7 @@ static inline void pci_restore_pasid_state(struct pci_dev *pdev) { }
 #ifdef CONFIG_PCI_IOV
 int pci_iov_init(struct pci_dev *dev);
 void pci_iov_release(struct pci_dev *dev);
+void pci_iov_disable(struct pci_dev *dev);
 void pci_iov_remove(struct pci_dev *dev);
 void pci_iov_update_resource(struct pci_dev *dev, int resno);
 resource_size_t pci_sriov_resource_alignment(struct pci_dev *dev, int resno);
@@ -977,6 +978,7 @@ static inline int pci_iov_init(struct pci_dev *dev)
 	return -ENODEV;
 }
 static inline void pci_iov_release(struct pci_dev *dev) { }
+static inline void pci_iov_disable(struct pci_dev *dev) { }
 static inline void pci_iov_remove(struct pci_dev *dev) { }
 static inline void pci_iov_update_resource(struct pci_dev *dev, int resno) { }
 static inline resource_size_t pci_sriov_resource_alignment(struct pci_dev *dev,
diff --git a/include/linux/pci.h b/include/linux/pci.h
index 1c270f1d512301de4d462fe7e5097c32af5c6f8d..859f767b30f726bd157a6080f5977c17c4827a1d 100644
--- a/include/linux/pci.h
+++ b/include/linux/pci.h
@@ -1015,6 +1015,13 @@ struct module;
  *		how to manage the DMA themselves and set this flag so that
  *		the IOMMU layer will allow them to setup and manage their
  *		own I/O address space.
+ * @managed_sriov: Disable SR-IOV on remove().
+ *		If set, the Single Root I/O Virtualization (SR-IOV)
+ *		capability of the device is disabled when the driver is
+ *		unbound from the device, by calling sriov_configure()
+ *		before remove(). The presence of this flag guarantees
+ *		that when a Virtual Function (VF) is bound to a driver,
+ *		the Physical Function (PF) is bound to a driver, too.
  */
 struct pci_driver {
 	const char		*name;
@@ -1033,6 +1040,7 @@ struct pci_driver {
 	struct device_driver	driver;
 	struct pci_dynids	dynids;
 	bool driver_managed_dma;
+	bool managed_sriov;
 };
 
 #define to_pci_driver(__drv)	\

-- 
2.53.0

