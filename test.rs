impl ToGraphString for MoneyV2 {
  fn to_graph_string(&self) -> String {
      match self {
      
          MoneyV2::Amount => stringify!(amount).to_string(),
      
          MoneyV2::CurrencyCode => stringify!(currencyCode).to_string(),
      }}}
Final code: impl ToGraphString for PriceRangeV2 {
  fn to_graph_string(&self) -> String {
      match self {
      
          PriceRangeV2::MaxVariantPrice(vector_var) => {
              let mut s = String::new();
              s.push_str("maxVariantPrice");
              s.push_str(" {");
              for field in vector_var.iter().enumerate() {
                  s.push('\n');
                  s.push_str(&field.1.to_graph_string());
              }
              s.push_str("\n }");
              s
          }
      
          PriceRangeV2::MinVariantPrice(vector_var) => {
              let mut s = String::new();
              s.push_str("minVariantPrice");
              s.push_str(" {");
              for field in vector_var.iter().enumerate() {
                  s.push('\n');
                  s.push_str(&field.1.to_graph_string());
              }
              s.push_str("\n }");
              s
          }
      }}}
Final code: impl ToGraphString for Image {
  fn to_graph_string(&self) -> String {
      match self {
      
          Image::AltText => stringify!(altText).to_string(),
      
          Image::Height => stringify!(height).to_string(),
      
          Image::Id => stringify!(id).to_string(),
      
          Image::Metafield {connector,namespace,key} => {
                  let mut output_string: Vec<String> = vec![];
                          output_string.push(format!("connector: {}", connector));
                      
                          output_string.push(format!("namespace: {}", namespace));
                      
                          output_string.push(format!("key: {}", key));
                      

                  output_string.join(", ")
              }, 
          
          Image::PrivateMetafield {connector,key,namespace} => {
                  let mut output_string: Vec<String> = vec![];
                          output_string.push(format!("connector: {}", connector));
                      
                          output_string.push(format!("key: {}", key));
                      
                          output_string.push(format!("namespace: {}", namespace));
                      

                  output_string.join(", ")
              }, 
          
          Image::Url => stringify!(url).to_string(),
      
          Image::Width => stringify!(width).to_string(),
      }}}
Final code: impl ToGraphString for Metafield {
  fn to_graph_string(&self) -> String {
      match self {
      
          Metafield::CreatedAt => stringify!(createdAt).to_string(),
      
          Metafield::Definition => stringify!(definition).to_string(),
      }}}
Final code: impl ToGraphString for MetafieldDefinition {
  fn to_graph_string(&self) -> String {
      match self {
      
          MetafieldDefinition::Description => stringify!(description).to_string(),
      
          MetafieldDefinition::Id => stringify!(id).to_string(),
      
          MetafieldDefinition::Key => stringify!(key).to_string(),
      
          MetafieldDefinition::MetafieldsCount {validation_status} => {
                  let mut output_string: Vec<String> = vec![];
                          if let Some(value) = validation_status {
                              output_string.push(format!("validation_status:{}", value));
                          }
                      

                  output_string.join(", ")
              }, 
          
          MetafieldDefinition::Name => stringify!(name).to_string(),
      
          MetafieldDefinition::Namespace => stringify!(namespace).to_string(),
      
          MetafieldDefinition::OwnerType => stringify!(ownerType).to_string(),
      
          MetafieldDefinition::PinnedPosition => stringify!(pinnedPosition).to_string(),
      }}}
Final code: impl ToGraphString for Object {
  fn to_graph_string(&self) -> String {
      match self {
      
          Object::AvailablePublicationCount => stringify!(availablePublicationCount).to_string(),
      
          Object::ContextualPricing(vector_var) => {
              let mut s = String::new();
              s.push_str("contextualPricing");
              s.push_str(" {");
              for field in vector_var.iter().enumerate() {
                  s.push('\n');
                  s.push_str(&field.1.to_graph_string());
              }
              s.push_str("\n }");
              s
          }
      
          Object::CreatedAt => stringify!(createdAt).to_string(),
      
          Object::DefaultCursor => stringify!(defaultCursor).to_string(),
      
          Object::Description {truncate_at} => {
                  let mut output_string: Vec<String> = vec![];
                          if let Some(value) = truncate_at {
                              output_string.push(format!("truncate_at:{}", value));
                          }
                      

                  output_string.join(", ")
              }, 
          
          Object::DescriptionHtml => stringify!(descriptionHtml).to_string(),
      
          Object::FeaturedImage => stringify!(featuredImage).to_string(),
      
          Object::GiftCardTemplateSuffix => stringify!(giftCardTemplateSuffix).to_string(),
      
          Object::Handle => stringify!(handle).to_string(),
      
          Object::HasOnlyDefaultVariant => stringify!(hasOnlyDefaultVariant).to_string(),
      
          Object::HasOutOfStockVariants => stringify!(hasOutOfStockVariants).to_string(),
      
          Object::Id => stringify!(id).to_string(),
      
          Object::InCollection {id} => {
                  let mut output_string: Vec<String> = vec![];
                          output_string.push(format!("id: {}", id));
                      

                  output_string.join(", ")
              }, 
          
          Object::IsGiftCard => stringify!(isGiftCard).to_string(),
      
          Object::LegacyResourceId => stringify!(legacyResourceId).to_string(),
      
          Object::MediaCount => stringify!(mediaCount).to_string(),
      
          Object::Metafield {connector,namespace,key} => {
                  let mut output_string: Vec<String> = vec![];
                          output_string.push(format!("connector: {}", connector));
                      
                          output_string.push(format!("namespace: {}", namespace));
                      
                          output_string.push(format!("key: {}", key));
                      

                  output_string.join(", ")
              }, 
          
          Object::OnlineStorePreviewUrl => stringify!(onlineStorePreviewUrl).to_string(),
      
          Object::Options {first} => {
                  let mut output_string: Vec<String> = vec![];
                          if let Some(value) = first {
                              output_string.push(format!("first:{}", value));
                          }
                      

                  output_string.join(", ")
              }, 
          
          Object::PriceRangeV2(vector_var) => {
              let mut s = String::new();
              s.push_str("priceRangeV2");
              s.push_str(" {");
              for field in vector_var.iter().enumerate() {
                  s.push('\n');
                  s.push_str(&field.1.to_graph_string());
              }
              s.push_str("\n }");
              s
          }
      
          Object::PrivateMetafield {connector,key,namespace} => {
                  let mut output_string: Vec<String> = vec![];
                          output_string.push(format!("connector: {}", connector));
                      
                          output_string.push(format!("key: {}", key));
                      
                          output_string.push(format!("namespace: {}", namespace));
                      

                  output_string.join(", ")
              }, 
          }}}
Final code: impl ToGraphString for ContextualPricing {
  fn to_graph_string(&self) -> String {
      match self {
      
          ContextualPricing::MaxVariantPricing(vector_var) => {
              let mut s = String::new();
              s.push_str("maxVariantPricing");
              s.push_str(" {");
              for field in vector_var.iter().enumerate() {
                  s.push('\n');
                  s.push_str(&field.1.to_graph_string());
              }
              s.push_str("\n }");
              s
          }
      
          ContextualPricing::MinVariantPricing(vector_var) => {
              let mut s = String::new();
              s.push_str("minVariantPricing");
              s.push_str(" {");
              for field in vector_var.iter().enumerate() {
                  s.push('\n');
                  s.push_str(&field.1.to_graph_string());
              }
              s.push_str("\n }");
              s
          }
      
          ContextualPricing::PriceRange(vector_var) => {
              let mut s = String::new();
              s.push_str("priceRange");
              s.push_str(" {");
              for field in vector_var.iter().enumerate() {
                  s.push('\n');
                  s.push_str(&field.1.to_graph_string());
              }
              s.push_str("\n }");
              s
          }
      }}}
Final code: impl ToGraphString for VariantContextualPricing {
  fn to_graph_string(&self) -> String {
      match self {
      
          VariantContextualPricing::CompareAtPrice(vector_var) => {
              let mut s = String::new();
              s.push_str("compareAtPrice");
              s.push_str(" {");
              for field in vector_var.iter().enumerate() {
                  s.push('\n');
                  s.push_str(&field.1.to_graph_string());
              }
              s.push_str("\n }");
              s
          }
      
          VariantContextualPricing::Price(vector_var) => {
              let mut s = String::new();
              s.push_str("price");
              s.push_str(" {");
              for field in vector_var.iter().enumerate() {
                  s.push('\n');
                  s.push_str(&field.1.to_graph_string());
              }
              s.push_str("\n }");
              s
          }
      }}}