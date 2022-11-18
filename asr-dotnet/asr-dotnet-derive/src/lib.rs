use proc_macro::TokenStream;
use quote::quote;
use syn::{Data, DeriveInput, Ident, Lit, Meta};

#[proc_macro_derive(MonoClassBinding, attributes(namespace))]
pub fn mono_class_binding(input: TokenStream) -> TokenStream {
    let ast: DeriveInput = syn::parse(input).unwrap();

    let struct_data = match ast.data {
        Data::Struct(s) => s,
        _ => panic!("Only structs are supported"),
    };

    let struct_name = ast.ident;
    let stuct_name_string = struct_name.to_string();

    let name_space_string = ast
        .attrs
        .iter()
        .find_map(|x| {
            let nv = match x.parse_meta().ok()? {
                Meta::NameValue(nv) => nv,
                _ => return None,
            };
            if nv.path.get_ident()? != "namespace" {
                return None;
            }
            match nv.lit {
                Lit::Str(s) => Some(s.value()),
                _ => None,
            }
        })
        .unwrap_or_default();
    let binding_name = Ident::new(&format!("{struct_name}Binding"), struct_name.span());

    let mut field_names = Vec::new();
    let mut field_name_strings = Vec::new();
    let mut field_types = Vec::new();
    for field in struct_data.fields {
        field_names.push(field.ident.clone().unwrap());
        field_name_strings.push(field.ident.clone().unwrap().to_string());
        field_types.push(field.ty);
    }

    #[cfg(not(feature = "il2cpp"))]
    {
        quote! {
            struct #binding_name {
                class: MonoClassDef,
                #(#field_names: usize,)*
            }

            impl #struct_name {
                fn bind(image: &MonoImage, process: &Process) -> Result<#binding_name, ()> {
                    let class = image
                            .classes(process)
                            .find(|c| {
                                c.klass
                                    .name
                                    .read_str(process, |v| v == #stuct_name_string.as_bytes())
                                    && c.klass
                                        .name_space
                                        .read_str(process, |v| v == #name_space_string.as_bytes())
                            })
                            .ok_or(())?;

                    #(
                        let #field_names = class.find_field(process, #field_name_strings).ok_or(())?;
                    )*
                    Ok(#binding_name {
                        class,
                        #(#field_names,)*
                    })
                }
            }

            impl #binding_name {
                fn class(&self) -> &MonoClassDef {
                    &self.class
                }

                fn load(&self, process: &Process, instance: Ptr) -> Result<#struct_name, ()> {
                    self.class
                        .klass
                        .get_instance(
                            instance,
                            process,
                            |instance_data| {
                                Ok(#struct_name {#(
                                    #field_names: *bytemuck::from_bytes(
                                        instance_data
                                            .get(self.#field_names..).ok_or(())?
                                            .get(..core::mem::size_of::<#field_types>()).ok_or(())?,
                                    ),
                                )*})
                            },
                        )?
                }
            }
        }
        .into()
    }

    #[cfg(feature = "il2cpp")]
    {
        quote! {
            struct #binding_name {
                class: MonoClass,
                #(#field_names: i32,)*
            }

            impl #struct_name {
                fn bind(image: &MonoImage, process: &Process, mono_module: &MonoModule) -> Result<#binding_name, ()> {
                    let class = image
                            .classes(process, mono_module)?
                            .find(|c| {
                                c
                                    .name
                                    .read_str(process, |v| v == #stuct_name_string.as_bytes())
                                    && c
                                        .name_space
                                        .read_str(process, |v| v == #name_space_string.as_bytes())
                            })
                            .ok_or(())?;

                    #(
                        let #field_names = class.find_field(process, #field_name_strings).ok_or(())?;
                    )*
                    Ok(#binding_name {
                        class,
                        #(#field_names,)*
                    })
                }
            }

            impl #binding_name {
                fn class(&self) -> &MonoClass {
                    &self.class
                }

                fn load(&self, process: &Process, instance: Ptr) -> Result<#struct_name, ()> {
                    self.class
                        .get_instance(
                            instance,
                            process,
                            |instance_data| {
                                Ok(#struct_name {#(
                                    #field_names: *bytemuck::from_bytes(
                                        instance_data
                                            .get(self.#field_names as usize..).ok_or(())?
                                            .get(..core::mem::size_of::<#field_types>()).ok_or(())?,
                                    ),
                                )*})
                            },
                        )?
                }
            }
        }
        .into()
    }
}
