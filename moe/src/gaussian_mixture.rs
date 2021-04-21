use crate::{MoeError, Result};
use linfa::{traits::*, Float};
use ndarray::{s, Array, Array1, Array2, Array3, ArrayBase, Axis, Data, Ix2, Ix3, Zip};
use ndarray_linalg::{cholesky::*, triangular::*, Lapack, Scalar};
use ndarray_stats::QuantileExt;

// def score_samples(self, X):
// """Compute the weighted log probabilities for each sample.
// Parameters
// ----------
// X : array-like of shape (n_samples, n_features)
//     List of n_features-dimensional data points. Each row
//     corresponds to a single data point.
// Returns
// -------
// log_prob : array, shape (n_samples,)
//     Log probabilities of each data point in X.
// """
// check_is_fitted(self)
// X = _check_X(X, None, self.means_.shape[1])

// return logsumexp(self._estimate_weighted_log_prob(X), axis=1)

// def score(self, X, y=None):
// """Compute the per-sample average log-likelihood of the given data X.
// Parameters
// ----------
// X : array-like of shape (n_samples, n_dimensions)
//     List of n_features-dimensional data points. Each row
//     corresponds to a single data point.
// Returns
// -------
// log_likelihood : float
//     Log likelihood of the Gaussian mixture given X.
// """
// return self.score_samples(X).mean()

// /// Return the number of free parameters in the model.
// fn n_parameters(n_clusters: usize, gmm: &GaussianMixtureModel<f64>) -> usize {
//     let n_features = gmm.means().shape()[1];
//     let cov_params = n_clusters * n_features * (n_features + 1) / 2;
//     let mean_params = n_features * n_clusters;
//     return (cov_params + mean_params + n_clusters - 1) as usize;
// }

// /// Bayesian information criterion for the current model on the input X.
// /// The lower the better.
// fn bic(x: &ArrayBase<impl Data<Elem = f64>, Ix2>) -> f64 {
//     -2 * self.score(X) * X.shape()[0] + n_parameters() * X.shape()[0].ln()
// }

// /// Akaike information criterion for the current model on the input X.
// /// The lower the better
// fn aic(x: &ArrayBase<impl Data<Elem = f64>, Ix2>) -> f64 {
//     -2 * self.score(X) * X.shape()[0] + 2 * n_parameters()
// }

pub struct GaussianMixture<F: Float> {
    weights: Array1<F>,
    means: Array2<F>,
    covariances: Array3<F>,
    precisions: Array3<F>,
    precisions_chol: Array3<F>,
    heaviside_factor: F,
}

impl<F: Float> Clone for GaussianMixture<F> {
    fn clone(&self) -> Self {
        Self {
            weights: self.weights.to_owned(),
            means: self.means.to_owned(),
            covariances: self.covariances.to_owned(),
            precisions: self.precisions.to_owned(),
            precisions_chol: self.precisions_chol.to_owned(),
            heaviside_factor: self.heaviside_factor,
        }
    }
}

impl<F: Float + Lapack + Scalar> GaussianMixture<F> {
    pub fn new(
        weights: Array1<F>,
        means: Array2<F>,
        covariances: Array3<F>,
    ) -> Result<GaussianMixture<F>> {
        let precisions_chol = Self::compute_precisions_cholesky_full(&covariances)?;
        let precisions = Self::compute_precisions_full(&precisions_chol);

        Ok(GaussianMixture {
            weights,
            means,
            covariances,
            precisions,
            precisions_chol,
            heaviside_factor: F::one(),
        })
    }

    pub fn weights(&self) -> &Array1<F> {
        &self.weights
    }

    pub fn means(&self) -> &Array2<F> {
        &self.means
    }

    pub fn covariances(&self) -> &Array3<F> {
        &self.covariances
    }

    pub fn precisions(&self) -> &Array3<F> {
        &self.precisions
    }

    pub fn heaviside_factor(&self) -> F {
        self.heaviside_factor
    }

    pub fn predict_probas<D: Data<Elem = F>>(&self, observations: &ArrayBase<D, Ix2>) -> Array2<F> {
        let (_, log_resp) = self.estimate_log_prob_resp(observations);
        log_resp.mapv(|v| Scalar::exp(v))
    }

    pub fn with_heaviside_factor(mut self, heaviside_factor: F) -> Self {
        self.heaviside_factor = heaviside_factor;
        self
    }

    fn estimate_gaussian_parameters<D: Data<Elem = F>>(
        observations: &ArrayBase<D, Ix2>,
        resp: &Array2<F>,
        reg_covar: F,
    ) -> Result<(Array1<F>, Array2<F>, Array3<F>)> {
        let nk = resp.sum_axis(Axis(0));
        if nk.min().unwrap() < &(F::from(10.).unwrap() * F::epsilon()) {
            return Err(MoeError::EmptyCluster(format!(
              "Cluster #{} has no more point. Consider decreasing number of clusters or change initialization.",
              nk.argmin().unwrap() + 1
          )));
        }

        let nk2 = nk.to_owned().insert_axis(Axis(1));
        let means = resp.t().dot(observations) / nk2;
        let covariances =
            Self::estimate_gaussian_covariances_full(&observations, resp, &nk, &means, reg_covar);
        Ok((nk, means, covariances))
    }

    fn estimate_gaussian_covariances_full<D: Data<Elem = F>>(
        observations: &ArrayBase<D, Ix2>,
        resp: &Array2<F>,
        nk: &Array1<F>,
        means: &Array2<F>,
        reg_covar: F,
    ) -> Array3<F> {
        let n_clusters = means.nrows();
        let n_features = means.ncols();
        let mut covariances = Array::zeros((n_clusters, n_features, n_features));
        for k in 0..n_clusters {
            let diff = observations - &means.row(k);
            let m = &diff.t() * &resp.index_axis(Axis(1), k);
            let mut cov_k = m.dot(&diff) / nk[k];
            cov_k.diag_mut().mapv_inplace(|x| x + reg_covar);
            covariances.slice_mut(s![k, .., ..]).assign(&cov_k);
        }
        covariances
    }

    fn compute_precisions_cholesky_full<D: Data<Elem = F>>(
        covariances: &ArrayBase<D, Ix3>,
    ) -> Result<Array3<F>> {
        let n_clusters = covariances.shape()[0];
        let n_features = covariances.shape()[1];
        let mut precisions_chol = Array::zeros((n_clusters, n_features, n_features));
        for (k, covariance) in covariances.outer_iter().enumerate() {
            let cov_chol = covariance.cholesky(UPLO::Lower)?;
            let sol =
                cov_chol.solve_triangular(UPLO::Lower, Diag::NonUnit, &Array::eye(n_features))?;
            precisions_chol.slice_mut(s![k, .., ..]).assign(&sol.t());
        }
        Ok(precisions_chol)
    }

    fn compute_precisions_full<D: Data<Elem = F>>(
        precisions_chol: &ArrayBase<D, Ix3>,
    ) -> Array3<F> {
        let mut precisions = Array3::zeros(precisions_chol.dim());
        for (k, prec_chol) in precisions_chol.outer_iter().enumerate() {
            precisions
                .slice_mut(s![k, .., ..])
                .assign(&prec_chol.dot(&prec_chol.t()));
        }
        precisions
    }

    // Estimate log probabilities (log P(X)) and responsibilities for each sample.
    // Compute weighted log probabilities per component (log P(X)) and responsibilities
    // for each sample in X with respect to the current state of the model.
    fn estimate_log_prob_resp<D: Data<Elem = F>>(
        &self,
        observations: &ArrayBase<D, Ix2>,
    ) -> (Array1<F>, Array2<F>) {
        let weighted_log_prob = self.estimate_weighted_log_prob(&observations);
        let log_prob_norm = weighted_log_prob
            .mapv(|v| Scalar::exp(v))
            .sum_axis(Axis(1))
            .mapv(|v| Scalar::ln(v));
        let log_resp = weighted_log_prob - log_prob_norm.to_owned().insert_axis(Axis(1));
        (log_prob_norm, log_resp)
    }

    // Estimate weighted log probabilities for each samples wrt to the model
    fn estimate_weighted_log_prob<D: Data<Elem = F>>(
        &self,
        observations: &ArrayBase<D, Ix2>,
    ) -> Array2<F> {
        self.estimate_log_prob(&observations) + self.estimate_log_weights()
    }

    // Compute log probabilities for each samples wrt to the model which is gaussian
    fn estimate_log_prob<D: Data<Elem = F>>(&self, observations: &ArrayBase<D, Ix2>) -> Array2<F> {
        self.estimate_log_gaussian_prob(&observations)
    }

    // Compute the log LikelihoodComputation in case of the gaussian probabilities
    // log(P(X|Mean, Precision)) = -0.5*(d*ln(2*PI)-ln(det(Precision))-(X-Mean)^t.Precision.(X-Mean)
    fn estimate_log_gaussian_prob<D: Data<Elem = F>>(
        &self,
        observations: &ArrayBase<D, Ix2>,
    ) -> Array2<F> {
        let n_samples = observations.nrows();
        let n_features = observations.ncols();
        let means = self.means();
        let n_clusters = means.nrows();
        let factor = ndarray_rand::rand_distr::num_traits::Float::powf(
            self.heaviside_factor(),
            F::from(-0.5).unwrap(),
        );
        let precs = &self.precisions_chol * factor;
        // GmmCovarType = full
        // det(precision_chol) is half of det(precision)
        let log_det = Self::compute_log_det_cholesky_full(&precs, n_features);
        let mut log_prob: Array2<F> = Array::zeros((n_samples, n_clusters));
        Zip::indexed(means.genrows())
            .and(precs.outer_iter())
            .apply(|k, mu, prec_chol| {
                let diff = (&observations.to_owned() - &mu).dot(&prec_chol);
                log_prob
                    .slice_mut(s![.., k])
                    .assign(&diff.mapv(|v| v * v).sum_axis(Axis(1)))
            });
        log_prob.mapv(|v| {
            F::from(-0.5).unwrap()
                * (v + F::from(n_features as f64 * f64::ln(2. * std::f64::consts::PI)).unwrap())
        }) + log_det
    }

    fn compute_log_det_cholesky_full<D: Data<Elem = F>>(
        matrix_chol: &ArrayBase<D, Ix3>,
        n_features: usize,
    ) -> Array1<F> {
        let n_clusters = matrix_chol.shape()[0];
        let log_diags = &matrix_chol
            .to_owned()
            .into_shape((n_clusters, n_features * n_features))
            .unwrap()
            .slice(s![.., ..; n_features+1])
            .to_owned()
            .mapv(|v| Scalar::ln(v));
        log_diags.sum_axis(Axis(1))
    }

    fn estimate_log_weights(&self) -> Array1<F> {
        self.weights().mapv(|v| Scalar::ln(v))
    }
}

impl<F: Float + Lapack + Scalar, D: Data<Elem = F>> PredictRef<ArrayBase<D, Ix2>, Array1<usize>>
    for GaussianMixture<F>
{
    fn predict_ref(&self, observations: &ArrayBase<D, Ix2>) -> Array1<usize> {
        let (_, log_resp) = self.estimate_log_prob_resp(&observations);
        log_resp
            .mapv(|v| Scalar::exp(v))
            .map_axis(Axis(1), |row| row.argmax().unwrap())
    }
}

#[cfg(test)]
mod tests {
    // extern crate openblas_src;
    // extern crate intel_mkl_src;
    use super::*;
    // use approx::assert_abs_diff_eq;
    use ndarray::{array, Array, Array2};
    use ndarray_npy::write_npy;

    #[test]
    fn test_gaussian_mixture() {
        let weights = array![0.5, 0.5];
        let means = array![[0., 0.], [4., 4.]];
        let covs = array![[[3., 0.], [0., 3.]], [[3., 0.], [0., 3.]]];
        let gmix = GaussianMixture::new(weights, means, covs)
            .expect("Gaussian mixture creation failed")
            .with_heaviside_factor(0.99);
        // let obs = array![[0., 0.], [1., 1.], [2., 2.], [3., 3.], [4., 4.]];
        // let n = 100;
        let mut obs = Array2::from_elem((101, 2), 0.);
        Zip::from(obs.genrows_mut())
            .and(&Array::linspace(0., 4., 101))
            .apply(|mut o, &v| o.assign(&array![v, v]));
        let preds = gmix.predict(&obs);
        println!("preds = {:?}", preds);
        let probas = gmix.predict_probas(&obs);
        println!("probas = {:?}", probas);
        write_npy("probes.npy", &obs).expect("probes saved");
        write_npy("probas.npy", &probas).expect("probas saved");
    }
}
